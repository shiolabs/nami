//! What home-manager itself generates (its `home-files`), for everything the
//! mappings don't cover: built with Nix, then read. Units in
//! ~/.config/systemd/user become your services; any other file becomes a
//! home file, copied next to the nami file, with store paths pointing at
//! nami's packages. A file that still needs something only Nix builds is
//! listed. What the mappings already wrote (shells, git, tmux, ssh, GTK...)
//! stays as they wrote it: readable, and without store paths.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};

use super::evaluated::Conv;
use super::{Found, Reader};
use crate::nix::expr::Val;

/// Files home-manager writes that nami does its own way (shell startup, its
/// session variables), or that are home-manager's own bookkeeping.
const OURS: &[&str] = &[
    ".zshrc", ".zshenv", ".zprofile", ".zlogin", ".zlogout", ".bashrc", ".bash_profile", ".profile", ".bash_logout",
    ".config/fish/config.fish", ".config/environment.d/10-home-manager.conf", ".cache/.keep", ".manpath",
    ".config/fontconfig/conf.d/10-hm-fonts.conf", ".config/nix/nix.conf",
    // nix-direnv's hook: nami's own project hook stands in for it.
    ".config/direnv/lib/hm-nix-direnv.sh",
];
const OURS_DIRS: &[&str] = &[".local/state/home-manager", ".local/state/nix", ".nix-profile", ".config/home-manager"];

/// One generated file or directory, read out of the store.
pub enum Item {
    File(Vec<u8>),
    Dir(PathBuf),
    /// A path in a package (one the binary cache has): not read.
    Package(PathBuf),
    /// What Nix couldn't build, and why.
    Missing(String),
}

/// `home-files`' top-level entries, by path relative to ~: each a file, or
/// a directory (a recursive source) taken whole.
#[cfg(test)]
pub fn read(out: &Path) -> BTreeMap<String, Item> {
    use super::nixtool::{canonical, real};
    let mut items = BTreeMap::new();
    // Paths as Nix sees them (its store may be a private one, see nixtool).
    fn walk(dir: &Path, rel: &str, items: &mut BTreeMap<String, Item>) {
        let on_disk = real(dir);
        for e in fs::read_dir(&on_disk).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let p = dir.join(&name);
            let link = fs::symlink_metadata(e.path()).map(|m| m.file_type().is_symlink()).unwrap_or(false);
            match fs::metadata(real(&p)) {
                // A link to a directory is a source taken whole; a real one, keep going.
                Ok(m) if m.is_dir() && link => {
                    items.insert(r, Item::Dir(canonical(&p)));
                }
                Ok(m) if m.is_dir() => walk(&p, &r, items),
                Ok(_) => {
                    if let Ok(bytes) = fs::read(real(&p)) {
                        items.insert(r, Item::File(bytes));
                    }
                }
                Err(_) => {}
            }
        }
    }
    walk(out, "", &mut items);
    items
}

fn covered(found: &Found, fish: bool, rel: &str) -> bool {
    OURS.contains(&rel)
        // home-manager's way of making a directory exist.
        || rel == ".keep" || rel.ends_with("/.keep")
        || OURS_DIRS.iter().any(|d| rel == *d || rel.starts_with(&format!("{d}/")))
        || found.files.keys().any(|k| rel == k || rel.starts_with(&format!("{k}/")) || k.starts_with(&format!("{rel}/")))
        // nami writes ~/.gitconfig from programs.git; home-manager ~/.config/git/config.
        || (rel == ".config/git/config" && found.files.contains_key(".gitconfig"))
        // fish functions are in shell.fish.
        || (rel.starts_with(".config/fish/functions/") && fish)
}

/// A unit file's sections: section -> [(key, value)].
fn sections(text: &str) -> BTreeMap<String, Vec<(String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut cur = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cur = s.to_string();
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.entry(cur.clone()).or_default().push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

impl Reader {
    /// Text with store paths, as Conv sees it: exact once nami installs the
    /// packages it names.
    fn retarget_text(conv: &mut Conv, text: &str) -> Option<String> {
        match conv.val(&serde_json::Value::String(text.to_string()), &mut Vec::new()) {
            Val::Str(s, true) => Some(s),
            _ => None,
        }
    }

    /// A generated script (a unit's ExecStart): its body, if it's a shell
    /// script nami can carry over.
    fn script_of(conv: &mut Conv, path: &str) -> Option<String> {
        let text = match conv.texts.get(path) {
            Some(t) => t.clone(),
            None => fs::read_to_string(super::nixtool::real(Path::new(path))).ok()?,
        };
        let mut lines = text.lines();
        let first = lines.next()?;
        if !(first.starts_with("#!") && (first.contains("/bash") || first.contains("/sh"))) {
            return None;
        }
        Self::retarget_text(conv, &lines.collect::<Vec<_>>().join("\n"))
    }

    /// A home-manager unit file as a `services:` entry of yours.
    fn generated_unit(&mut self, conv: &mut Conv, name: &str, text: &str, timer: Option<&str>) -> Result<Value, String> {
        let s = |v: &str| Value::String(v.to_string());
        let mut m = Mapping::new();
        let mut raw: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        let mut env = Mapping::new();
        let mut installed = false;
        for (section, keys) in sections(text) {
            for (k, v) in keys {
                // A generated script as the command (no package of nami's): its body.
                if section == "Service" && k == "ExecStart" && !m.contains_key("run") && !m.contains_key("script") {
                    let cmd = v.trim_start_matches(['-', '@', '+', '!', ':']);
                    let is_package = Self::retarget_text(conv, cmd).is_some_and(|r| r != cmd);
                    if cmd.starts_with('/') && !cmd.contains(char::is_whitespace) && !is_package {
                        if let Some(body) = Self::script_of(conv, cmd) {
                            m.insert(s("script"), s(&body));
                            continue;
                        }
                    }
                }
                let v = Self::retarget_text(conv, &v).ok_or_else(|| format!("{section}.{k} refers to something only Nix builds ({v})"))?;
                match (section.as_str(), k.as_str()) {
                    ("Unit", "Description") => {
                        m.insert(s("description"), s(&v));
                    }
                    ("Service", "ExecStart") => {
                        let cmd = v.trim_start_matches(['-', '@', '+', '!', ':']).to_string();
                        if m.contains_key("run") || m.contains_key("script") {
                            raw.entry("Service".into()).or_default().entry("ExecStart".into()).or_default().push(cmd);
                        } else {
                            m.insert(s("run"), s(&cmd));
                        }
                    }
                    ("Service", "Environment") => {
                        for kv in v.split_whitespace() {
                            if let Some((n, val)) = kv.trim_matches('"').split_once('=') {
                                env.insert(s(n), s(val));
                            }
                        }
                    }
                    ("Install", "WantedBy") if v.split_whitespace().all(|t| t == "default.target") => installed = true,
                    ("Install", "WantedBy") => {
                        installed = true;
                        raw.entry("Install".into()).or_default().entry(k).or_default().push(v);
                    }
                    ("Service", "Type") if v == "simple" || v == "exec" => {}
                    _ => raw.entry(section.clone()).or_default().entry(k).or_default().push(v),
                }
            }
        }
        if m.get("run").and_then(Value::as_str).is_some_and(|r| r.contains('%')) {
            return Err("uses systemd's % specifiers in ExecStart".into());
        }
        if !m.contains_key("run") && !m.contains_key("script") {
            return Err("no ExecStart".into());
        }
        if let Some(t) = timer {
            let tsec = sections(t);
            match tsec.get("Timer").and_then(|keys| keys.iter().find(|(k, _)| k == "OnCalendar")) {
                Some((_, when)) => {
                    m.insert(s("at"), s(when));
                    installed = true;
                    for (k, v) in tsec.get("Timer").into_iter().flatten().filter(|(k, _)| k != "OnCalendar" && k != "Unit") {
                        raw.entry("Timer".into()).or_default().entry(k.clone()).or_default().push(v.clone());
                    }
                }
                None => return Err(format!("{name}.timer isn't a calendar timer")),
            }
        }
        if !env.is_empty() {
            m.insert(s("env"), Value::Mapping(env));
        }
        if !installed {
            m.insert(s("enable"), Value::Bool(false));
        }
        if !raw.is_empty() {
            let sections: Mapping = raw
                .into_iter()
                .map(|(sec, keys)| (s(&sec), Value::Mapping(keys.into_iter().map(|(k, v)| (s(&k), if v.len() == 1 { s(&v[0]) } else { Value::Sequence(v.iter().map(|x| s(x)).collect()) })).collect())))
                .collect();
            m.insert(s("systemd"), Value::Mapping(sections));
        }
        Ok(Value::Mapping(m))
    }

    /// Everything in home-manager's built `home-files` the mappings didn't
    /// cover: units as services, the rest as files (`Found::generated`).
    pub(super) fn read_generated(&mut self, items: BTreeMap<String, Item>, conv: &mut Conv) {
        let units_dir = ".config/systemd/user/";
        let fish = self.shells.has("fish");
        // zsh plugins home-manager fetched: it put them in ~/.zsh/plugins.
        for (name, file) in self.shells.fetched_plugins() {
            let dir = format!(".zsh/plugins/{name}");
            if items.contains_key(&dir) || items.keys().any(|k| k.starts_with(&format!("{dir}/"))) {
                self.shells.plugin_from_home(&name, &file);
                self.found.unconverted.retain(|u| u.what != format!("programs.zsh.plugins.{name}"));
            }
        }
        for (rel, item) in &items {
            if covered(&self.found, fish, rel) {
                continue;
            }
            if let Some(unit) = rel.strip_prefix(units_dir) {
                // `<target>.wants/` links: the units say where they're wanted.
                if unit.contains('/') {
                    continue;
                }
                let Item::File(bytes) = item else { continue };
                let text = String::from_utf8_lossy(bytes).into_owned();
                let what = format!("~/{rel}");
                if let Some(name) = unit.strip_suffix(".service") {
                    if self.found.user_services.contains_key(name) || name.starts_with("nami-") {
                        continue;
                    }
                    let timer = match items.get(&format!("{units_dir}{name}.timer")) {
                        Some(Item::File(t)) => Some(String::from_utf8_lossy(t).into_owned()),
                        _ => None,
                    };
                    match self.generated_unit(conv, name, &text, timer.as_deref()) {
                        Ok(v) => self.add_service(name, v, &what, true),
                        Err(why) => self.found.skip(what, format!("a home-manager unit: {why}")),
                    }
                } else if unit.ends_with(".timer") && items.contains_key(&format!("{units_dir}{}.service", unit.trim_end_matches(".timer"))) {
                    // read with its service
                } else if unit.ends_with(".target") {
                    // A target only groups units (home-manager's nami.target for
                    // nami applets, hm-graphical-session.target): nothing to run.
                } else {
                    self.found.skip(what, "a systemd unit that isn't a service or its timer: nami's services are .service units");
                }
                continue;
            }
            match item {
                // Text is UTF-8 without NULs; anything else is copied byte for byte.
                Item::File(bytes) => match std::str::from_utf8(bytes).ok().filter(|t| !t.contains('\0')).ok_or(()) {
                    Ok(text) if text.contains("/nix/store/") => match Self::retarget_text(conv, text) {
                        Some(t) => {
                            self.found.generated.insert(rel.clone(), Generated::Text(t));
                        }
                        None => self.found.skip(format!("~/{rel}"), "generated by home-manager, and it refers to something only Nix builds"),
                    },
                    Ok(text) => {
                        self.found.generated.insert(rel.clone(), Generated::Text(text.to_string()));
                    }
                    Err(_) if bytes.windows(11).any(|w| w == b"/nix/store/") => self.found.skip(format!("~/{rel}"), "a binary file with Nix store paths in it"),
                    Err(_) => {
                        self.found.generated.insert(rel.clone(), Generated::Bytes(bytes.clone()));
                    }
                },
                Item::Missing(why) => self.found.skip(format!("~/{rel}"), format!("generated by home-manager, but Nix couldn't build it here ({why})")),
                Item::Package(src) => {
                    // The package installed; the file linked from nami's
                    // profile, unless the profile is already where it's
                    // looked for (an icon theme in XDG_DATA_DIRS, a cursor
                    // in XCURSOR_PATH).
                    let shown = src.display().to_string();
                    match Self::retarget_text(conv, &shown) {
                        Some(p) if p != shown => {
                            for pkg in std::mem::take(&mut conv.used) {
                                self.found.package(&pkg, &format!("~/{rel}"));
                            }
                            let tail = |prefix: &str| rel.strip_prefix(prefix).map(|t| shown.ends_with(&format!("/share/{t}")) || shown.ends_with(&format!("/share/icons/{t}")));
                            if !(tail(".local/share/") == Some(true) || tail(".icons/") == Some(true)) {
                                self.found.generated.insert(rel.clone(), Generated::Link(p));
                            }
                        }
                        _ => self.found.skip(format!("~/{rel}"), format!("generated by home-manager from a package nami doesn't have ({shown})")),
                    }
                }
                Item::Dir(src) => {
                    // A package's directory (an icon or cursor theme home-manager
                    // links into ~): the package installed is the same, and nami's
                    // profile is where XDG_DATA_DIRS and XCURSOR_PATH look.
                    let shown = src.display().to_string();
                    match Self::retarget_text(conv, &shown) {
                        Some(p) if p != shown => {
                            for pkg in std::mem::take(&mut conv.used) {
                                self.found.package(&pkg, &format!("~/{rel}"));
                            }
                        }
                        _ => {
                            self.found.generated.insert(rel.clone(), Generated::Dir(src.clone()));
                        }
                    }
                }
            }
        }
        // Packages the files and units point at.
        for p in std::mem::take(&mut conv.used) {
            self.found.package(&p, "a store path in a generated file");
        }
    }
}

/// A generated file, to copy next to the nami file.
#[derive(Debug, Clone)]
pub enum Generated {
    Text(String),
    Bytes(Vec<u8>),
    /// A directory in the store, copied whole.
    Dir(PathBuf),
    /// A file or directory in a package: linked from nami's profile.
    Link(String),
}

/// Write the generated files under `dir` (the nami file's `files/`).
pub fn write(generated: &BTreeMap<String, Generated>, dir: &Path) -> std::io::Result<()> {
    // `src` as Nix sees it (see nixtool): links inside followed the same way.
    fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
        use super::nixtool::real;
        fs::create_dir_all(dest)?;
        for e in fs::read_dir(real(src))? {
            let e = e?;
            let from = src.join(e.file_name());
            let to = dest.join(e.file_name());
            if fs::metadata(real(&from))?.is_dir() {
                copy_dir(&from, &to)?;
            } else {
                fs::copy(real(&from), &to)?;
                use std::os::unix::fs::PermissionsExt;
                let exec = fs::metadata(&to)?.permissions().mode() & 0o111;
                fs::set_permissions(&to, fs::Permissions::from_mode(0o644 | exec))?;
            }
        }
        Ok(())
    }
    for (rel, g) in generated {
        let dest = dir.join(rel);
        fs::create_dir_all(dest.parent().unwrap())?;
        match g {
            Generated::Text(t) => fs::write(&dest, t)?,
            Generated::Bytes(b) => fs::write(&dest, b)?,
            Generated::Dir(src) => copy_dir(src, &dest)?,
            Generated::Link(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Catalog, Tier};
    use super::*;

    #[test]
    fn home_files_become_services_and_files() {
        let dir = std::env::temp_dir().join(format!("nami-generated-{}", std::process::id()));
        let h = "0".repeat(32);
        let store = dir.join("store");
        let out = dir.join("home-files");
        let w = |p: &Path, s: &[u8]| {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, s).unwrap();
        };
        // A generated start script, and a package's theme directory.
        let script = store.join(format!("{h}-udiskie-start"));
        w(&script, format!("#!/nix/store/{h}-bash-5.3/bin/bash\nexec /nix/store/{h}-udiskie-2.5/bin/udiskie --nami\n").as_bytes());
        let theme = PathBuf::from(format!("/nix/store/{h}-papirus-icon-theme-1.0/share/icons/Papirus"));
        w(&out.join(".config/systemd/user/udiskie.service"), format!("[Unit]\nDescription=udiskie\nAfter=graphical-session.target\n[Service]\nExecStart={}\nRestart=always\n[Install]\nWantedBy=graphical-session.target\n", script.display()).as_bytes());
        w(&out.join(".config/systemd/user/backup.service"), format!("[Service]\nType=oneshot\nExecStart=/nix/store/{h}-restic-0.18/bin/restic backup /home\n").as_bytes());
        w(&out.join(".config/systemd/user/backup.timer"), b"[Timer]\nOnCalendar=daily\nPersistent=true\n[Install]\nWantedBy=timers.target\n");
        w(&out.join(".config/systemd/user/nami.target"), b"[Unit]\nDescription=nami applets\n");
        w(&out.join(".config/bat/config"), b"--theme=TwoDark\n");
        w(&out.join(".config/mpv/mpv.conf"), format!("script=/nix/store/{h}-mpv-mpris-1.1/share/mpv/scripts/mpris.so\n").as_bytes());
        w(&out.join(".config/x/needs-nix"), format!("source /nix/store/{h}-hm-session-vars.sh\n").as_bytes());
        w(&out.join(".local/share/x.db"), b"\x00\x01binary");
        w(&out.join(".zshrc"), b"# home-manager's\n");
        w(&out.join(".cache/.keep"), b"");
        fs::create_dir_all(out.join(".icons")).unwrap();

        let catalog = Catalog::new(["udiskie", "restic", "mpv-mpris", "papirus-icon-theme"]);
        let mut conv = Conv::new(Some(&catalog), Tier::User);
        let mut r = Reader::new(Some(Tier::User));
        // The script's path must read as itself, not a package: point Conv's
        // check at the fake store by reading it through script_of directly.
        let body = Reader::script_of(&mut conv, &script.display().to_string()).unwrap();
        assert_eq!(body, "exec udiskie --nami");
        r.read_generated(read(&out), &mut conv);
        let f = &r.found;
        // The unit with a script of its own became `script:`; units name their targets.
        let udiskie = &f.user_services["udiskie"];
        assert_eq!(udiskie["script"], "exec udiskie --nami");
        assert_eq!(udiskie["restart"].as_str(), None, "Restart= stays a systemd key: {udiskie:?}");
        assert_eq!(udiskie["systemd"]["Install"]["WantedBy"], "graphical-session.target");
        assert_eq!(udiskie["systemd"]["Unit"]["After"], "graphical-session.target");
        let backup = &f.user_services["backup"];
        assert_eq!((backup["run"].as_str(), backup["at"].as_str()), (Some("restic backup /home"), Some("daily")));
        assert_eq!(backup["systemd"]["Timer"]["Persistent"], "true");
        // Files: as written, store paths pointing at nami's profile; what needs Nix listed.
        assert!(matches!(&f.generated[".config/bat/config"], Generated::Text(t) if t == "--theme=TwoDark\n"));
        let home = std::env::var("HOME").unwrap();
        assert!(matches!(&f.generated[".config/mpv/mpv.conf"], Generated::Text(t) if *t == format!("script={home}/.local/state/nami/profile/share/mpv/scripts/mpris.so\n")));
        assert!(matches!(f.generated[".local/share/x.db"], Generated::Bytes(_)));
        assert!(f.unconverted.iter().any(|u| u.what == "~/.config/x/needs-nix"), "{:?}", f.unconverted);
        assert!(!f.generated.contains_key(".zshrc") && !f.generated.contains_key(".cache/.keep"));
        assert!(!f.unconverted.iter().any(|u| u.what.contains("nami.target")));
        let pkgs: Vec<&str> = f.packages.iter().map(|(a, _)| a.as_str()).collect();
        assert!(pkgs.contains(&"mpv-mpris"), "{pkgs:?}");
        let _ = theme;
        fs::remove_dir_all(&dir).unwrap();
    }
}
