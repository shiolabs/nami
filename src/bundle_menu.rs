//! A bundle in the app menu: its entry, as the bundle's runtime makes it
//! (`--nami-bundle-install`, on any Linux distribution) and as namiOS does when it
//! installs one (src/bundle_app.rs). Both name it `nami-bundle-<id>.desktop`
//! in ~/.local/share/applications, its icon `nami-bundle-<id>` in the
//! person's hicolor icons, so one app never gets two entries; one namiOS
//! made says so (`X-Tray-App=<its id>`), and the runtime leaves it be.
//! Bundles made before the rename named theirs `tray-bundle-<id>`: where
//! that one is there, it's the app's entry, and stays its name.
//! Only std: the runtime has nothing else.

// Shared by nami and the runtime: each uses its own half.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
}

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// The entry's id: `nami-bundle-<the app's own id>`, or `tray-bundle-<id>`
/// where a bundle made before the rename put its entry under that name.
pub fn entry_id(desktop_id: &str) -> String {
    entry_id_in(&data_home(), desktop_id)
}

/// The same, in the data folder `data` (~/.local/share).
pub fn entry_id_in(data: &Path, desktop_id: &str) -> String {
    let id = desktop_id.trim_end_matches(".desktop");
    let [nami, tray] = entry_ids(id);
    let apps = data.join("applications");
    if !apps.join(format!("{nami}.desktop")).exists() && apps.join(format!("{tray}.desktop")).exists() {
        return tray;
    }
    nami
}

/// Both names an app's entry may have: nami's, and tray's before the rename.
pub fn entry_ids(desktop_id: &str) -> [String; 2] {
    let id = desktop_id.trim_end_matches(".desktop");
    [format!("nami-bundle-{id}"), format!("tray-bundle-{id}")]
}

/// Where its entry goes.
pub fn entry_file(desktop_id: &str) -> PathBuf {
    data_home().join("applications").join(format!("{}.desktop", entry_id(desktop_id)))
}

/// Where its icon goes: a picture of the bundle's, `.png` or `.svg`.
pub fn icon_file(desktop_id: &str, picture: &str) -> Option<PathBuf> {
    icon_file_in(&data_home(), desktop_id, picture)
}

/// The same, in the data folder `data` (~/.local/share). (A symbolic one,
/// a single colour to be drawn in the text's, keeps saying so.)
pub fn icon_file_in(data: &Path, desktop_id: &str, picture: &str) -> Option<PathBuf> {
    let ext = Path::new(picture).extension()?.to_string_lossy().to_lowercase();
    let dir = match ext.as_str() {
        "svg" => "scalable/apps",
        "png" => "256x256/apps",
        _ => return None,
    };
    let symbolic = if is_symbolic(picture) { "-symbolic" } else { "" };
    Some(data.join("icons/hicolor").join(dir).join(format!("{}{symbolic}.{ext}", entry_id_in(data, desktop_id))))
}

/// A symbolic icon (`…-symbolic.svg`): one colour, drawn in the text's.
pub fn is_symbolic(picture: &str) -> bool {
    Path::new(picture).file_stem().is_some_and(|s| s.to_string_lossy().ends_with("-symbolic"))
}

/// How good a picture `rel` (a path under a store path or data folder:
/// `share/icons/<theme>/<size>/<kind>/<file>`, `share/pixmaps/<file>`) is for
/// the icon named `name`: None if it isn't one. Bigger is better: a full
/// colour one over a symbolic one, the app's own (hicolor) over a theme's,
/// big over small, an app's over others'.
pub fn icon_score(rel: &str, name: &str) -> Option<u32> {
    let rel = rel.strip_prefix("share/").unwrap_or(rel);
    let file = rel.rsplit('/').next()?;
    let (stem, ext) = file.rsplit_once('.')?;
    if !["png", "svg"].contains(&ext) || name.is_empty() || name.contains('/') {
        return None;
    }
    let symbolic = if stem == name {
        false
    } else if stem.strip_suffix("-symbolic") == Some(name) {
        true
    } else {
        return None;
    };
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() == 2 && parts[0] == "pixmaps" {
        return Some(if symbolic { 500 } else { 4100 });
    }
    // icons/<theme>/<a>/<b>/<file>: a size and a kind, either way round.
    if parts.len() != 5 || parts[0] != "icons" {
        return None;
    }
    let px = |s: &str| -> Option<u32> {
        if s == "scalable" || s == "symbolic" {
            return Some(256);
        }
        let s = s.split('@').next()?;
        let (w, h) = s.split_once('x')?;
        (w == h).then(|| w.parse::<u32>().ok()).flatten().map(|n| n.min(256))
    };
    let (size, kind) = match (px(parts[2]), px(parts[3])) {
        (Some(n), _) => (n, parts[3]),
        (None, Some(n)) => (n, parts[2]),
        _ => return None,
    };
    let svg = if ext == "svg" { 40 } else { 0 };
    Some(if symbolic { 0 } else { 4000 } + if parts[1] == "hicolor" { 300 } else { 0 } + size + svg + if kind == "apps" { 50 } else { 0 })
}

/// A key of a desktop entry's [Desktop Entry] group.
pub fn entry_key(text: &str, k: &str) -> Option<String> {
    let mut inside = false;
    for l in text.lines() {
        if l.starts_with('[') {
            inside = l.trim() == "[Desktop Entry]";
        } else if inside {
            if let Some((key, v)) = l.split_once('=') {
                if key.trim() == k {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// The program an entry's Exec starts (its quotes undone).
pub fn exec_program(text: &str) -> Option<String> {
    let e = entry_key(text, "Exec")?;
    Some(match e.strip_prefix('"') {
        Some(rest) => {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => out.extend(chars.next()),
                    c => out.push(c),
                }
            }
            out
        }
        None => e.split_whitespace().next().unwrap_or("").to_string(),
    })
}

/// The kinds of file and links an entry opens (`MimeType=`).
pub fn kinds(text: &str) -> Vec<String> {
    entry_key(text, "MimeType").unwrap_or_default().split(';').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect()
}

/// The links it opens (`x-scheme-handler/<scheme>`), but the web's, mail's and files'.
pub fn link_kinds(text: &str) -> Vec<String> {
    kinds(text).into_iter().filter(|k| k.strip_prefix("x-scheme-handler/").is_some_and(|s| !["http", "https", "ftp", "mailto", "file"].contains(&s))).collect()
}

/// mimeapps.list's `[Default Applications]`: `entry` set as the default
/// for each of `kinds` that has none (`add`), or taken out where it is
/// the default (not `add`).
pub fn defaults(text: &str, kinds: &[String], entry: &str, add: bool) -> String {
    let has = |k: &str| {
        let mut inside = false;
        text.lines().any(|l| {
            if l.starts_with('[') {
                inside = l.trim() == "[Default Applications]";
                return false;
            }
            inside && l.split_once('=').is_some_and(|(key, v)| key.trim() == k && !v.trim().is_empty())
        })
    };
    let missing: Vec<&String> = kinds.iter().filter(|k| add && !has(k)).collect();
    let mut out: Vec<String> = Vec::new();
    let (mut inside, mut seen) = (false, false);
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if inside {
                out.extend(missing.iter().map(|k| format!("{k}={entry}")));
            }
            inside = t == "[Default Applications]";
            seen |= inside;
        } else if inside && !add && t.split_once('=').is_some_and(|(k, v)| kinds.iter().any(|x| x == k.trim()) && v.trim().trim_end_matches(';') == entry) {
            continue;
        }
        out.push(l.to_string());
    }
    if inside {
        out.extend(missing.iter().map(|k| format!("{k}={entry}")));
    } else if !seen && !missing.is_empty() {
        if out.last().is_some_and(|l| !l.is_empty()) {
            out.push(String::new());
        }
        out.push("[Default Applications]".into());
        out.extend(missing.iter().map(|k| format!("{k}={entry}")));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Exec's program, quoted as the desktop entry spec says.
pub fn exec_quote(exe: &str) -> String {
    if exe.contains([' ', '"', '\'', '\\', '$', '`', '%']) {
        format!("\"{}\"", exe.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$").replace('`', "\\`").replace('%', "%%"))
    } else {
        exe.to_string()
    }
}

/// The entry for the menu, made from the app's own: started from the
/// bundle at `exe`, its icon `icon` (a name or a path), not D-Bus activated
/// (no service of it is installed), known to the dock by its own id
/// (StartupWMClass, where it says none), with `marks` added.
pub fn menu_entry(file: &str, exe: &str, icon: Option<&str>, own_id: &str, marks: &[(&str, &str)]) -> String {
    let quoted = exec_quote(exe);
    let mut text = String::new();
    let (mut in_entry, mut class) = (false, false);
    let end_entry = |text: &mut String, class: bool| {
        // (Before the blank line between groups.)
        let blank = text.ends_with("\n\n");
        if blank {
            text.pop();
        }
        if !class && !own_id.is_empty() {
            text.push_str(&format!("StartupWMClass={own_id}\n"));
        }
        for (k, v) in marks {
            text.push_str(&format!("{k}={v}\n"));
        }
        if blank {
            text.push('\n');
        }
    };
    for line in file.lines() {
        if line.starts_with('[') {
            if in_entry {
                end_entry(&mut text, class);
            }
            in_entry = line.trim() == "[Desktop Entry]";
        }
        let kv = line.split_once('=').map(|(k, v)| (k.trim(), v.trim()));
        let line = match kv {
            // The program's own arguments (%U, --new-window) stay.
            Some(("Exec", v)) => format!("Exec={quoted}{}", v.split_once(' ').map(|(_, r)| format!(" {r}")).unwrap_or_default()),
            Some(("TryExec", _)) => continue,
            Some((k, _)) if in_entry && marks.iter().any(|(m, _)| *m == k) => continue,
            Some(("DBusActivatable", _)) => "DBusActivatable=false".into(),
            Some(("Icon", _)) if in_entry && icon.is_some() => format!("Icon={}", icon.unwrap_or_default()),
            Some(("StartupWMClass", _)) if in_entry => {
                class = true;
                line.to_string()
            }
            _ => line.to_string(),
        };
        text += &line;
        text.push('\n');
    }
    if in_entry {
        end_entry(&mut text, class);
    }
    text
}

/// The menu's own list of what opens what, brought up to date, as package
/// managers do (where there's nothing to do it, desktops read the entries).
pub fn refresh_menu(apps: &Path) {
    let found = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("update-desktop-database")).find(|f| f.is_file()));
    if let Some(u) = found {
        let null = std::process::Stdio::null;
        let _ = std::process::Command::new(u).arg("-q").arg(apps).stdin(null()).stdout(null()).stderr(null()).status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = "[Desktop Entry]\nName=Demo\nExec=demo --new %U\nTryExec=demo\nIcon=demo\nDBusActivatable=true\nMimeType=text/plain;x-scheme-handler/demo;x-scheme-handler/https;\n\n[Desktop Action new]\nName=New\nExec=demo --window\nIcon=demo\n";

    #[test]
    fn an_entry_made_before_the_rename_keeps_its_name() {
        let d = std::env::temp_dir().join(format!("nami-bundle-entry-{}", std::process::id()));
        let apps = d.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        assert_eq!(entry_id_in(&d, "org.x.App.desktop"), "nami-bundle-org.x.App");
        // tray's runtime put it in the menu: that's its entry, and its icon's name.
        std::fs::write(apps.join("tray-bundle-org.x.App.desktop"), "").unwrap();
        assert_eq!(entry_id_in(&d, "org.x.App"), "tray-bundle-org.x.App");
        assert_eq!(icon_file_in(&d, "org.x.App", "/s/x.svg"), Some(d.join("icons/hicolor/scalable/apps/tray-bundle-org.x.App.svg")));
        // Both there: nami's.
        std::fs::write(apps.join("nami-bundle-org.x.App.desktop"), "").unwrap();
        assert_eq!(entry_id_in(&d, "org.x.App"), "nami-bundle-org.x.App");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entries_start_the_bundle() {
        let t = menu_entry(ENTRY, "/home/a b/apps/demo/demo-x86_64", Some("nami-bundle-demo"), "org.demo.Demo", &[("X-Tray-App", "demo")]);
        assert!(t.contains("Exec=\"/home/a b/apps/demo/demo-x86_64\" --new %U\n"), "{t}");
        assert!(t.contains("Exec=\"/home/a b/apps/demo/demo-x86_64\" --window\n"), "{t}");
        assert!(!t.contains("TryExec"));
        assert!(t.contains("DBusActivatable=false"));
        // The main group's icon and marks; an action's icon as it was.
        assert!(t.contains("Icon=nami-bundle-demo\nDBusActivatable=false\nMimeType=text/plain;x-scheme-handler/demo;x-scheme-handler/https;\nStartupWMClass=org.demo.Demo\nX-Tray-App=demo\n\n[Desktop Action new]"), "{t}");
        assert!(t.ends_with("Icon=demo\n"), "{t}");
        assert_eq!(exec_program(&t).as_deref(), Some("/home/a b/apps/demo/demo-x86_64"));
        assert_eq!(entry_key(&t, "X-Tray-App").as_deref(), Some("demo"));
        assert_eq!(link_kinds(ENTRY), vec!["x-scheme-handler/demo"]);
    }

    #[test]
    fn links_it_opens_are_its_where_nothing_else_opens_them() {
        let k = vec!["x-scheme-handler/demo".to_string(), "x-scheme-handler/other".to_string()];
        let text = "[Added Associations]\na=b\n\n[Default Applications]\nx-scheme-handler/other=else.desktop\n";
        let added = defaults(text, &k, "nami-bundle-demo.desktop", true);
        assert_eq!(added, "[Added Associations]\na=b\n\n[Default Applications]\nx-scheme-handler/other=else.desktop\nx-scheme-handler/demo=nami-bundle-demo.desktop\n");
        assert_eq!(defaults(&added, &k, "nami-bundle-demo.desktop", false), text);
        assert_eq!(defaults("", &k[..1], "e.desktop", true), "[Default Applications]\nx-scheme-handler/demo=e.desktop\n");
    }

    #[test]
    fn icons_by_their_name() {
        let n = "applications-games";
        assert_eq!(icon_score("share/icons/Adwaita/symbolic/categories/x.svg", n), None);
        let sym = icon_score("share/icons/Adwaita/symbolic/categories/applications-games-symbolic.svg", n).unwrap();
        let small = icon_score("share/icons/Adwaita/48x48/apps/applications-games.png", n).unwrap();
        let big = icon_score("share/icons/Adwaita/256x256/apps/applications-games.png", n).unwrap();
        let own = icon_score("share/icons/hicolor/48x48/apps/applications-games.png", n).unwrap();
        // (Papirus's way round: kind, then size.)
        assert!(icon_score("icons/Papirus/apps/64x64/applications-games.svg", n).is_some());
        assert!(sym < small && small < big && big < own, "{sym} {small} {big} {own}");
        assert!(icon_score("share/pixmaps/applications-games.png", n).unwrap() > sym);
        assert!(is_symbolic("/x/icon-symbolic.svg") && !is_symbolic("/x/icon.svg"));
        assert_eq!(icon_file_in(Path::new("/d"), "a.desktop", "/s/x-symbolic.svg"), Some(PathBuf::from("/d/icons/hicolor/scalable/apps/nami-bundle-a-symbolic.svg")));
    }
}
