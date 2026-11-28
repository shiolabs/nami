//! A bundle's optional sandbox (`nami bundle --sandbox`, project.nami's
//! `bundle: sandbox:`): the program runs inside bubblewrap (bwrap) where the
//! computer has it, seeing the system read-only, a home of its own, its
//! window, and only what its developer declared:
//!
//! ```yaml
//! bundle:
//!   sandbox: { network: true, files: [documents, downloads], devices: [gpu, audio] }
//! ```
//!
//! What it gets always: the system's files read-only (/usr, /etc, …: its
//! libraries are the bundle's own anyway), its window (the Wayland socket,
//! or X11's, which can't be fenced), its own data in
//! `~/.local/share/nami-bundle/sandbox/<app>` (its home inside), the
//! person's look (GTK's settings, fonts, icons) read-only, and the files it's
//! opened with. Where xdg-dbus-proxy is there too (it comes with Flatpak), it
//! talks to the desktop's portals alone, as a Flatpak app does, so its Open
//! and Save dialogs reach the person's files through them.
//!
//! Without bwrap (or where it can't make a sandbox) the bundle runs as it
//! would without one and says so once: a bundle's promise is that it runs on
//! any distribution, and a sandbox the person didn't ask for mustn't break
//! that. `NAMI_BUNDLE_SANDBOX=require` refuses instead.
//!
//! Shared by nami (which reads the declaration and describes it) and the
//! runtime (which builds bwrap's arguments): std only beyond bundle_format.

// Shared by nami and the runtime: each uses its own half.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::bundle_format::Sandbox;

/// The person's folders a bundle may ask for: (short name, its
/// xdg-user-dirs name, its usual folder, as people say it).
pub const FILES: &[(&str, &str, &str, &str)] = &[
    ("documents", "DOCUMENTS", "Documents", "Documents"),
    ("downloads", "DOWNLOAD", "Downloads", "Downloads"),
    ("music", "MUSIC", "Music", "Music"),
    ("pictures", "PICTURES", "Pictures", "Pictures"),
    ("videos", "VIDEOS", "Videos", "Videos"),
    ("desktop", "DESKTOP", "Desktop", "Desktop"),
    ("home", "", "", "home"),
];

/// The devices a bundle may ask for, and how people say them.
pub const DEVICES: &[(&str, &str)] = &[("gpu", "the graphics card"), ("audio", "your speakers and microphone"), ("camera", "the camera")];

/// The words `--sandbox` takes, and what `sandbox:` takes as keys.
pub const KEYS: &[&str] = &["network", "files", "devices"];

fn all_names() -> Vec<&'static str> {
    let mut v = vec!["network"];
    v.extend(FILES.iter().map(|f| f.0));
    v.extend(DEVICES.iter().map(|d| d.0));
    v
}

/// `--sandbox network,documents,gpu`: each word a right (empty: none).
pub fn parse_list(text: &str) -> Result<Sandbox, String> {
    let mut s = Sandbox::default();
    for w in text.split([',', ' ']).map(str::trim).filter(|w| !w.is_empty()) {
        add(&mut s, w)?;
    }
    Ok(s)
}

fn add(s: &mut Sandbox, w: &str) -> Result<(), String> {
    let w = w.to_ascii_lowercase();
    if w == "network" {
        s.network = true;
    } else if FILES.iter().any(|f| f.0 == w) {
        if !s.files.contains(&w) {
            s.files.push(w);
        }
    } else if DEVICES.iter().any(|d| d.0 == w) {
        if !s.devices.contains(&w) {
            s.devices.push(w);
        }
    } else {
        return Err(format!("the sandbox has no right `{w}`: it takes {}", all_names().join(", ")));
    }
    Ok(())
}

/// project.nami's `bundle: sandbox:`: `true` (no rights beyond its window),
/// `false` (none), or `{ network: true, files: [documents], devices: [gpu] }`;
/// read as JSON (the runtime has serde_json, and no YAML reader to carry).
pub fn from_value(v: &serde_json::Value, what: &str) -> Result<Option<Sandbox>, String> {
    use serde_json::Value;
    let list = |v: &Value, key: &str, names: &[&str]| -> Result<Vec<String>, String> {
        let items: Vec<String> = match v {
            Value::Null => Vec::new(),
            Value::String(s) => vec![s.clone()],
            Value::Array(items) => items.iter().map(|i| i.as_str().map(str::to_string).ok_or_else(|| format!("{what}: bundle: sandbox: {key}: a list of names ({})", names.join(", ")))).collect::<Result<_, _>>()?,
            _ => return Err(format!("{what}: bundle: sandbox: {key}: a list of names ({})", names.join(", "))),
        };
        for i in &items {
            if !names.contains(&i.as_str()) {
                return Err(format!("{what}: bundle: sandbox: {key}: there's no {i:?}; it takes {}", names.join(", ")));
            }
        }
        Ok(items)
    };
    match v {
        Value::Null | Value::Bool(false) => Ok(None),
        Value::Bool(true) => Ok(Some(Sandbox::default())),
        Value::Object(m) => {
            let mut s = Sandbox::default();
            for (k, val) in m {
                match k.as_str() {
                    "network" => s.network = val.as_bool().ok_or_else(|| format!("{what}: bundle: sandbox: network: true or false"))?,
                    "files" => s.files = list(val, "files", &FILES.iter().map(|f| f.0).collect::<Vec<_>>())?,
                    "devices" => s.devices = list(val, "devices", &DEVICES.iter().map(|d| d.0).collect::<Vec<_>>())?,
                    other => return Err(format!("{what}: bundle: sandbox: has no key `{other}`; it takes {}", KEYS.join(", "))),
                }
            }
            Ok(Some(s))
        }
        _ => Err(format!("{what}: bundle: sandbox: true, or what it may use: {{ network: true, files: [documents], devices: [gpu] }}")),
    }
}

/// "a, b and c".
fn and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What it may use, as people say it: "the network, your Documents folder
/// and the graphics card" (empty: nothing).
pub fn rights(s: &Sandbox) -> String {
    let mut parts: Vec<String> = Vec::new();
    if s.network {
        parts.push("the network".into());
    }
    if s.files.iter().any(|f| f == "home") {
        parts.push("your home folder".into());
    } else {
        let folders: Vec<String> = FILES.iter().filter(|f| s.files.iter().any(|x| x == f.0)).map(|f| f.3.to_string()).collect();
        match folders.len() {
            0 => {}
            1 => parts.push(format!("your {} folder", folders[0])),
            _ => parts.push(format!("your {} folders", and(&folders))),
        }
    }
    parts.extend(DEVICES.iter().filter(|d| s.devices.iter().any(|x| x == d.0)).map(|d| d.1.to_string()));
    and(&parts)
}

/// What the person is told of it, where it can run one or not.
pub fn words(s: Option<&Sandbox>, here: bool) -> String {
    match (s, here) {
        (None, _) => "Not sandboxed: it can use everything you can.".into(),
        (Some(s), true) => match rights(s) {
            r if r.is_empty() => "Runs in a sandbox: it can't use the network or your files.".into(),
            r => format!("Runs in a sandbox: it can use {r}."),
        },
        (Some(_), false) => "Made to run in a sandbox, but this computer can't make one (it needs bubblewrap). Not sandboxed: it can use everything you can.".into(),
    }
}

/// The app's own folder inside: its id, safe as a file name.
pub fn app_id(meta: &crate::bundle_format::Meta) -> String {
    let id = meta.desktop.as_ref().map(|d| d.id.trim_end_matches(".desktop").to_string()).unwrap_or_else(|| meta.name.clone());
    id.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

/// A D-Bus-style app id for the portals (three parts or more): its own when
/// its desktop file has one (org.gnome.TextEditor), else made from its name.
pub fn portal_id(meta: &crate::bundle_format::Meta) -> String {
    let id = app_id(meta);
    let ok = id.split('.').count() >= 3 && id.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !p.starts_with(|c: char| c.is_ascii_digit()));
    if ok { id } else { format!("io.nami.bundle.{}", id.replace(['.', '-'], "_")) }
}

/// bwrap, where the computer has it.
pub fn find_bwrap() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    std::env::split_paths(&path).chain(["/usr/bin".into(), "/bin".into()]).map(|d| d.join("bwrap")).find(|f| f.is_file())
}

/// What a path on the host is.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Missing,
    Dir,
    File,
    /// A link, and where it points.
    Link(PathBuf),
}

pub fn kind(p: &Path) -> Kind {
    match std::fs::symlink_metadata(p) {
        Err(_) => Kind::Missing,
        Ok(m) if m.file_type().is_symlink() => std::fs::read_link(p).map(Kind::Link).unwrap_or(Kind::Missing),
        Ok(m) if m.is_dir() => Kind::Dir,
        Ok(_) => Kind::File,
    }
}

/// Everything the sandbox is made from: worked out from the computer by
/// the runtime, kept apart so the arguments can be tested.
pub struct Setup<'a> {
    pub sandbox: &'a Sandbox,
    /// A window: the program gets its own session (no typing into the terminal).
    pub gui: bool,
    pub home: PathBuf,
    /// Its own home inside, kept between runs.
    pub data: PathBuf,
    /// The bundles' shared store, seen at `prefix` (/tmp/<5 letters>), and
    /// the cache it's in (the host libraries' links, the fonts' file).
    pub store_dir: PathBuf,
    pub prefix: String,
    pub cache_root: PathBuf,
    /// XDG_RUNTIME_DIR, and the sockets there: Wayland's, PulseAudio's, PipeWire's.
    pub runtime_dir: Option<PathBuf>,
    pub wayland: Option<PathBuf>,
    /// X11's socket (/tmp/.X11-unix/X0) and the person's Xauthority file.
    pub x11: Option<PathBuf>,
    pub xauthority: Option<PathBuf>,
    /// The person's folders by their short names (documents -> ~/Documents).
    pub folders: Vec<(String, PathBuf)>,
    /// Device files for gpu and camera (/dev/nvidia0, /dev/video0, …).
    pub nvidia: Vec<PathBuf>,
    pub cameras: Vec<PathBuf>,
    /// Files and folders it's started with: each in the sandbox, writable.
    pub opened: Vec<PathBuf>,
    /// The filtered session bus (xdg-dbus-proxy's socket), and the file
    /// that tells the portals it's sandboxed (/.flatpak-info).
    pub bus: Option<PathBuf>,
    pub info: Option<PathBuf>,
    /// Kept open while it runs (the proxy goes when it closes).
    pub sync_fd: Option<i32>,
    pub look: &'a dyn Fn(&Path) -> Kind,
}

/// The person's look, read-only inside: GTK's and Qt's settings, fonts, icons, themes.
const LOOK: &[&str] = &[".config/gtk-3.0", ".config/gtk-4.0", ".config/fontconfig", ".config/kdeglobals", ".config/Kvantum", ".config/qt5ct", ".config/qt6ct", ".config/user-dirs.dirs", ".config/user-dirs.locale", ".local/share/fonts", ".fonts", ".local/share/icons", ".icons", ".local/share/themes", ".themes", ".Xresources"];

/// The top folders of the system, read-only (links as links, for /lib -> usr/lib), and nami's
/// own, /nami (what /nix/var/nami and /run/current-system lead to; a link where it stayed there).
const SYSTEM: &[&str] = &["/usr", "/etc", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/opt", "/nix", "/nami"];

/// bwrap's arguments, before `--` and the program.
pub fn args(s: &Setup) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |items: &[&dyn AsRef<std::ffi::OsStr>]| a.extend(items.iter().map(|i| i.as_ref().to_os_string()));
    push(&[&"--unshare-all", &"--die-with-parent"]);
    if s.sandbox.network {
        push(&[&"--share-net"]);
    }
    if s.gui {
        push(&[&"--new-session"]);
    }
    let look = s.look;
    let system = |p: &Path, push: &mut dyn FnMut(&[&dyn AsRef<std::ffi::OsStr>])| match look(p) {
        Kind::Dir => push(&[&"--ro-bind", &p, &p]),
        Kind::File => push(&[&"--ro-bind", &p, &p]),
        Kind::Link(t) => push(&[&"--symlink", &t, &p]),
        Kind::Missing => {}
    };
    for d in SYSTEM {
        system(Path::new(d), &mut push);
    }
    push(&[&"--proc", &"/proc", &"--dev", &"/dev", &"--tmpfs", &"/tmp", &"--tmpfs", &"/run", &"--dir", &"/var/tmp"]);
    for d in ["/sys/block", "/sys/bus", "/sys/class", "/sys/dev", "/sys/devices"] {
        push(&[&"--ro-bind-try", &d, &d]);
    }
    // NixOS's and namiOS's (the bundle's parts it shares link there), and name lookups.
    let mut run: Vec<&str> = vec!["/run/current-system", "/run/opengl-driver", "/run/opengl-driver-32"];
    if s.sandbox.network {
        run.extend(["/run/systemd/resolve", "/run/NetworkManager", "/run/resolvconf", "/run/nscd"]);
    }
    for d in run {
        system(Path::new(d), &mut push);
    }
    // Its home: its own folder, or the person's when it may use all of it.
    let whole_home = s.sandbox.files.iter().any(|f| f == "home");
    if whole_home {
        push(&[&"--bind", &s.home, &s.home]);
    } else {
        push(&[&"--bind", &s.data, &s.home]);
        for rel in LOOK {
            let p = s.home.join(rel);
            if look(&p) != Kind::Missing {
                push(&[&"--ro-bind-try", &p, &p]);
            }
        }
        for (name, dir) in &s.folders {
            if s.sandbox.files.iter().any(|f| f == name) && matches!(look(dir), Kind::Dir) {
                push(&[&"--bind", dir, dir]);
            }
        }
    }
    // The bundle itself: the shared store where its files say it is.
    push(&[&"--ro-bind", &s.cache_root, &s.cache_root, &"--ro-bind", &s.store_dir, &s.prefix]);
    // Its window.
    if let Some(rt) = &s.runtime_dir {
        push(&[&"--dir", rt]);
        if let Some(w) = &s.wayland {
            push(&[&"--ro-bind", w, w]);
        }
        // Files opened through the portals.
        push(&[&"--bind-try", &rt.join("doc"), &rt.join("doc")]);
        if s.sandbox.devices.iter().any(|d| d == "audio") {
            for sock in ["pulse/native", "pipewire-0"] {
                push(&[&"--ro-bind-try", &rt.join(sock), &rt.join(sock)]);
            }
        }
        if let Some(bus) = &s.bus {
            push(&[&"--ro-bind", bus, &rt.join("bus")]);
        }
    }
    if let Some(x) = &s.x11 {
        push(&[&"--ro-bind", x, x]);
        if let Some(auth) = &s.xauthority {
            push(&[&"--ro-bind-try", auth, auth]);
        }
    }
    if let Some(info) = &s.info {
        push(&[&"--ro-bind", info, &"/.flatpak-info"]);
    }
    for d in &s.sandbox.devices {
        match d.as_str() {
            "gpu" => {
                push(&[&"--dev-bind-try", &"/dev/dri", &"/dev/dri"]);
                for n in &s.nvidia {
                    push(&[&"--dev-bind-try", n, n]);
                }
            }
            "audio" => push(&[&"--dev-bind-try", &"/dev/snd", &"/dev/snd"]),
            "camera" => {
                for c in &s.cameras {
                    push(&[&"--dev-bind-try", c, c]);
                }
            }
            _ => {}
        }
    }
    // What it's opened with, last (on top of what's above).
    for p in &s.opened {
        if p != &s.home && p != Path::new("/") {
            push(&[&"--bind-try", p, p]);
        }
    }
    if let Some(fd) = s.sync_fd {
        push(&[&"--sync-fd", &fd.to_string()]);
    }
    a
}

/// The person's folders (~/.config/user-dirs.dirs, else the usual names).
pub fn folders(home: &Path, config: &Path) -> Vec<(String, PathBuf)> {
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    FILES
        .iter()
        .filter(|f| !f.1.is_empty())
        .map(|(name, xdg, usual, _)| {
            let said = text.lines().find_map(|l| l.trim().strip_prefix(&format!("XDG_{xdg}_DIR=")).map(|v| v.trim().trim_matches('"').to_string()));
            let dir = match said {
                Some(v) if v.starts_with("$HOME/") => home.join(&v[6..]),
                Some(v) if v.starts_with('/') => PathBuf::from(v),
                _ => home.join(usual),
            };
            (name.to_string(), dir)
        })
        // (A folder that is the home itself, as xdg-user-dirs says for one turned off, isn't one.)
        .filter(|(_, d)| d != home)
        .collect()
}

/// The files and folders among its arguments (paths, file:// links): what
/// the person opened it with, to be reachable inside.
pub fn opened(args: &[OsString], cwd: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in args {
        let Some(t) = a.to_str() else { continue };
        let p = if let Some(rest) = t.strip_prefix("file://") {
            PathBuf::from(percent_decode(rest.trim_start_matches("localhost")))
        } else if t.starts_with('-') || t.contains("://") || t.is_empty() {
            continue;
        } else {
            cwd.join(t)
        };
        if p.exists() && !out.contains(&p) {
            out.push(std::fs::canonicalize(&p).unwrap_or(p));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// /.flatpak-info: what tells the desktop's portals the app is sandboxed
/// (so a file picked in Open is handed to it through the document portal).
pub fn flatpak_info(portal_id: &str, s: &Sandbox) -> String {
    let mut shared = vec!["ipc"];
    if s.network {
        shared.push("network");
    }
    format!("[Application]\nname={portal_id}\nruntime=runtime/io.nami.Bundle/{}/1\n\n[Context]\nshared={};\nsockets=wayland;x11;\n\n[Instance]\ninstance-id=nami-bundle\n", std::env::consts::ARCH, shared.join(";"))
}

/// xdg-dbus-proxy's rules: the portals, and its own name.
pub fn proxy_rules(portal_id: &str) -> Vec<String> {
    vec![
        "--filter".into(),
        format!("--own={portal_id}"),
        format!("--own={portal_id}.*"),
        "--talk=org.freedesktop.portal.*".into(),
        "--call=org.freedesktop.portal.*=*".into(),
        "--broadcast=org.freedesktop.portal.*=@/org/freedesktop/portal/*".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rights_from_words_and_yaml() {
        let s = parse_list("network, documents,downloads gpu").unwrap();
        assert_eq!(s, Sandbox { network: true, files: vec!["documents".into(), "downloads".into()], devices: vec!["gpu".into()] });
        assert_eq!(parse_list("").unwrap(), Sandbox::default());
        assert!(parse_list("network,printer").unwrap_err().contains("no right `printer`"));
        // (As bundle.rs reads project.nami: its YAML as JSON.)
        let y = |t: &str| serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(t).unwrap()).unwrap();
        let s = from_value(&y("{ network: true, files: [documents, downloads], devices: [gpu, audio] }"), "project.nami").unwrap().unwrap();
        assert!(s.network);
        assert_eq!(s.files, ["documents", "downloads"]);
        assert_eq!(s.devices, ["gpu", "audio"]);
        assert_eq!(from_value(&y("true"), "p").unwrap(), Some(Sandbox::default()));
        assert_eq!(from_value(&y("false"), "p").unwrap(), None);
        assert!(from_value(&y("{ files: [secrets] }"), "p").unwrap_err().contains("there's no \"secrets\""));
        assert!(from_value(&y("{ net: true }"), "p").unwrap_err().contains("no key `net`"));
    }

    #[test]
    fn said_plainly() {
        let s = Sandbox { network: true, files: vec!["documents".into()], devices: vec![] };
        assert_eq!(words(Some(&s), true), "Runs in a sandbox: it can use the network and your Documents folder.");
        let s = Sandbox { network: false, files: vec!["documents".into(), "downloads".into()], devices: vec!["gpu".into(), "audio".into()] };
        assert_eq!(words(Some(&s), true), "Runs in a sandbox: it can use your Documents and Downloads folders, the graphics card and your speakers and microphone.");
        assert_eq!(words(Some(&Sandbox::default()), true), "Runs in a sandbox: it can't use the network or your files.");
        assert_eq!(words(None, true), "Not sandboxed: it can use everything you can.");
        assert!(words(Some(&s), false).ends_with("Not sandboxed: it can use everything you can."));
    }

    fn setup<'a>(sb: &'a Sandbox, look: &'a dyn Fn(&Path) -> Kind) -> Setup<'a> {
        Setup {
            sandbox: sb,
            gui: true,
            home: "/home/ada".into(),
            data: "/home/ada/.local/share/nami-bundle/sandbox/org.x.App".into(),
            store_dir: "/home/ada/.cache/tray-bundle/store".into(),
            prefix: "/tmp/abcde".into(),
            cache_root: "/home/ada/.cache/tray-bundle".into(),
            runtime_dir: Some("/run/user/1000".into()),
            wayland: Some("/run/user/1000/wayland-1".into()),
            x11: None,
            xauthority: None,
            folders: vec![("documents".into(), "/home/ada/Documents".into()), ("downloads".into(), "/home/ada/Downloads".into())],
            nvidia: vec![],
            cameras: vec![],
            opened: vec!["/home/ada/Notes/todo.txt".into()],
            bus: None,
            info: None,
            sync_fd: None,
            look,
        }
    }

    fn pairs(a: &[OsString]) -> String {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn bwraps_arguments() {
        let look = |p: &Path| match p.to_str().unwrap() {
            "/lib" => Kind::Link("usr/lib".into()),
            "/usr" | "/etc" | "/nix" | "/nami" | "/home/ada/Documents" | "/home/ada/Downloads" | "/home/ada/.config/gtk-4.0" => Kind::Dir,
            _ => Kind::Missing,
        };
        let sb = Sandbox { network: false, files: vec!["documents".into()], devices: vec!["gpu".into()] };
        let a = pairs(&args(&setup(&sb, &look)));
        assert!(a.starts_with("--unshare-all --die-with-parent --new-session "), "{a}");
        assert!(!a.contains("--share-net"));
        // (nami's folder, /nami, beside the store: /nix/var/nami and /run/current-system lead there.)
        assert!(a.contains("--ro-bind /usr /usr --ro-bind /etc /etc --symlink usr/lib /lib --ro-bind /nix /nix --ro-bind /nami /nami --proc /proc"), "{a}");
        assert!(!a.contains("/lib64"), "missing ones are left out: {a}");
        // Its own home; the person's look read-only; Documents, not Downloads.
        assert!(a.contains("--bind /home/ada/.local/share/nami-bundle/sandbox/org.x.App /home/ada "), "{a}");
        assert!(a.contains("--ro-bind-try /home/ada/.config/gtk-4.0 /home/ada/.config/gtk-4.0"), "{a}");
        assert!(a.contains("--bind /home/ada/Documents /home/ada/Documents"), "{a}");
        assert!(!a.contains("Downloads"), "{a}");
        // The store where its files say it is.
        assert!(a.contains("--ro-bind /home/ada/.cache/tray-bundle/store /tmp/abcde"), "{a}");
        assert!(a.contains("--ro-bind /run/user/1000/wayland-1 /run/user/1000/wayland-1"), "{a}");
        assert!(a.contains("--dev-bind-try /dev/dri /dev/dri"), "{a}");
        assert!(!a.contains("pulse"), "no sound asked for: {a}");
        assert!(a.ends_with("--bind-try /home/ada/Notes/todo.txt /home/ada/Notes/todo.txt"), "{a}");
        // The network, and the whole home.
        let sb = Sandbox { network: true, files: vec!["home".into()], devices: vec!["audio".into()] };
        let a = pairs(&args(&setup(&sb, &look)));
        assert!(a.contains("--share-net"));
        assert!(a.contains("--bind /home/ada /home/ada"), "{a}");
        assert!(!a.contains("sandbox/org.x.App"), "{a}");
        assert!(a.contains("--ro-bind-try /run/user/1000/pulse/native /run/user/1000/pulse/native"), "{a}");
    }

    #[test]
    fn what_it_was_opened_with() {
        let dir = std::env::temp_dir().join(format!("nami-sb-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a b")).unwrap();
        std::fs::write(dir.join("a b/x.txt"), "").unwrap();
        let args: Vec<OsString> = ["--new-window", "a b/x.txt", "missing.txt", "https://example.org", &format!("file://{}/a%20b", dir.display())].iter().map(OsString::from).collect();
        let got = opened(&args, &dir);
        let canon = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(got, vec![canon.join("a b/x.txt"), canon.join("a b")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folders_as_xdg_says() {
        let dir = std::env::temp_dir().join(format!("nami-sbd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("user-dirs.dirs"), "XDG_DOCUMENTS_DIR=\"$HOME/Dokumente\"\nXDG_MUSIC_DIR=\"$HOME/\"\n").unwrap();
        let f = folders(Path::new("/home/ada"), &dir);
        assert!(f.contains(&("documents".into(), PathBuf::from("/home/ada/Dokumente"))));
        assert!(f.contains(&("downloads".into(), PathBuf::from("/home/ada/Downloads"))));
        assert!(!f.iter().any(|(n, _)| n == "music"), "turned off: the home itself");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn portal_ids() {
        let mut m = crate::bundle_format::Meta { name: "my-app".into(), ..Default::default() };
        assert_eq!(portal_id(&m), "io.nami.bundle.my_app");
        m.desktop = Some(crate::bundle_format::Desktop { id: "org.gnome.TextEditor.desktop".into(), file: String::new(), icon: None });
        assert_eq!(portal_id(&m), "org.gnome.TextEditor");
        assert!(flatpak_info("org.x.App", &Sandbox::default()).contains("name=org.x.App"));
    }
}
