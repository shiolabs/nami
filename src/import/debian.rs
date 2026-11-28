//! An Ubuntu or Debian computer, read from its files (its `/`, or an old
//! disk mounted somewhere): the apps the person installed (apt, snaps,
//! flatpaks), Wi-Fi networks, GNOME settings, shell, dotfiles, toolchains.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Result};
use serde_yaml::{Mapping, Value};

use super::dconf;
use super::moving::{add_wifi, secret, secret_name, secret_text, top, Apps, HomePlan, Match, Moved, Person, Wifi};

pub fn read(root: &Path, user: Option<&str>, apps: &Apps) -> Result<Moved> {
    let person = person(root, user)?;
    let dump = dconf_dump(root, &person);
    read_with(root, person, apps, dump)
}

// ---------- who ----------

struct Account {
    login: String,
    uid: u32,
    gid: u32,
    gecos: String,
    home: String,
    shell: String,
}

fn accounts(root: &Path) -> Vec<Account> {
    let text = std::fs::read_to_string(root.join("etc/passwd")).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 7).then(|| Account { login: f[0].into(), uid: f[2].parse().unwrap_or(0), gid: f[3].parse().unwrap_or(0), gecos: f[4].into(), home: f[5].into(), shell: f[6].into() })
        })
        .collect()
}

/// Under the root: `/home/alice` on a disk mounted at /mnt/old is /mnt/old/home/alice.
fn under(root: &Path, p: &str) -> PathBuf {
    root.join(p.trim_start_matches('/'))
}

fn is_person(root: &Path, a: &Account) -> bool {
    (1000..60000).contains(&a.uid) && !a.shell.ends_with("nologin") && !a.shell.ends_with("false") && under(root, &a.home).is_dir()
}

/// The people with a home on it.
pub fn people(root: &Path) -> Vec<String> {
    accounts(root).iter().filter(|a| is_person(root, a)).map(|a| a.login.clone()).collect()
}

fn person(root: &Path, user: Option<&str>) -> Result<Person> {
    let all = accounts(root);
    let people: Vec<&Account> = all.iter().filter(|a| is_person(root, a)).collect();
    let names = || people.iter().map(|a| a.login.as_str()).collect::<Vec<_>>().join(", ");
    let me = match user {
        Some(u) => all.iter().find(|a| a.login == u).ok_or_else(|| anyhow::anyhow!("no user {u} on {} (people there: {})", root.display(), names()))?,
        None => {
            // On this computer: whoever runs it (unless that's root).
            use std::os::unix::fs::MetadataExt;
            let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
            match all.iter().find(|a| a.uid == uid).filter(|_| root == Path::new("/") && uid != 0) {
                Some(a) => a,
                None => match people.as_slice() {
                    [one] => one,
                    [] => bail!("no one's home is on {}: say whose settings to bring with --user <name>", root.display()),
                    _ => bail!("several people use {}: say whose settings to bring with --user <name> ({})", root.display(), names()),
                },
            }
        }
    };
    let group_text = std::fs::read_to_string(root.join("etc/group")).unwrap_or_default();
    let mut groups = Vec::new();
    for l in group_text.lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() >= 4 && (f[3].split(',').any(|m| m == me.login) || f[2].parse::<u32>().ok() == Some(me.gid)) {
            groups.push(f[0].to_string());
        }
    }
    let name = me.gecos.split(',').next().map(str::trim).filter(|n| !n.is_empty() && *n != me.login).map(String::from);
    let admin = groups.iter().any(|g| ["sudo", "admin", "wheel"].contains(&g.as_str()));
    Ok(Person { login: me.login.clone(), name, home: under(root, &me.home), shell: Some(me.shell.clone()).filter(|s| !s.is_empty()), groups, admin })
}

/// `dconf dump /` of the person's settings: None without any (no GNOME), or why it couldn't be read.
fn dconf_dump(root: &Path, p: &Person) -> Option<std::result::Result<String, String>> {
    let db = p.home.join(".config/dconf/user");
    if !db.is_file() {
        return None;
    }
    let mut cmd = Command::new("dconf");
    cmd.args(["dump", "/"]);
    let me_here = root == Path::new("/") && std::env::var_os("HOME").is_some_and(|h| Path::new(&h) == p.home);
    if !me_here {
        // Someone else's (or an old disk's): read their database file itself.
        let profile = std::env::temp_dir().join(format!("nami-import-dconf-{}", std::process::id()));
        if std::fs::write(&profile, "user-db:user\n").is_err() {
            return Some(Err("couldn't write a dconf profile".into()));
        }
        cmd.env("DCONF_PROFILE", &profile).env("XDG_CONFIG_HOME", p.home.join(".config")).env_remove("DBUS_SESSION_BUS_ADDRESS");
        let out = cmd.output();
        let _ = std::fs::remove_file(&profile);
        return Some(dump_result(out));
    }
    Some(dump_result(cmd.output()))
}

fn dump_result(out: std::io::Result<std::process::Output>) -> std::result::Result<String, String> {
    match out {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(_) => Err("there's no dconf here to read them (`nami install dconf`, then import again)".into()),
    }
}

// ---------- dpkg ----------

#[derive(Default)]
struct Deb {
    priority: String,
    section: String,
    deps: Vec<String>,
}

/// Debian control stanzas: blank-line separated `Key: value`, with continuation lines.
fn stanzas(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut out = Vec::new();
    let mut cur: BTreeMap<String, String> = BTreeMap::new();
    let mut last = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(v) = cur.get_mut(&last) {
                v.push('\n');
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            last = k.trim().to_string();
            cur.insert(last.clone(), v.trim().to_string());
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `a (>= 1), b | c, d:any` -> [a, b, c, d].
fn dep_names(text: &str) -> Vec<String> {
    text.split([',', '|']).filter_map(|d| d.split_whitespace().next()).map(|n| n.split(':').next().unwrap_or(n).to_string()).filter(|n| !n.is_empty()).collect()
}

/// Installed packages, and the ones apt installed as something else's dependency.
fn dpkg(root: &Path) -> (BTreeMap<String, Deb>, BTreeSet<String>) {
    let mut installed = BTreeMap::new();
    for s in stanzas(&std::fs::read_to_string(root.join("var/lib/dpkg/status")).unwrap_or_default()) {
        let (Some(name), Some(status)) = (s.get("Package"), s.get("Status")) else { continue };
        if !status.ends_with(" installed") {
            continue;
        }
        let deps = ["Pre-Depends", "Depends", "Recommends"].iter().filter_map(|k| s.get(*k)).flat_map(|d| dep_names(d)).collect();
        installed.insert(name.clone(), Deb { priority: s.get("Priority").cloned().unwrap_or_default(), section: s.get("Section").cloned().unwrap_or_default(), deps });
    }
    let auto = stanzas(&std::fs::read_to_string(root.join("var/lib/apt/extended_states")).unwrap_or_default())
        .into_iter()
        .filter(|s| s.get("Auto-Installed").map(String::as_str) == Some("1"))
        .filter_map(|s| s.get("Package").cloned())
        .collect();
    (installed, auto)
}

/// What the system itself brought: the essential packages, the distro's
/// meta-packages (ubuntu-desktop, task-gnome-desktop...) and everything
/// they pull in, and what the installer put there (its manifest).
fn base(root: &Path, installed: &BTreeMap<String, Deb>) -> BTreeSet<String> {
    let meta = |n: &str, d: &Deb| {
        ["ubuntu-", "kubuntu-", "xubuntu-", "lubuntu-", "ubuntustudio-", "task-", "debian-", "pop-", "linuxmint-", "mint-", "elementary-", "zorin-"].iter().any(|p| n.starts_with(p))
            || d.section.ends_with("metapackages")
    };
    let mut todo: Vec<String> = installed.iter().filter(|(n, d)| ["required", "important", "standard"].contains(&d.priority.as_str()) || meta(n, d)).map(|(n, _)| n.clone()).collect();
    // The installer's list of what it installed.
    if let Ok(f) = std::fs::File::open(root.join("var/log/installer/initial-status.gz")) {
        let mut text = String::new();
        if std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(f), &mut text).is_ok() {
            todo.extend(stanzas(&text).into_iter().filter_map(|s| s.get("Package").cloned()));
        }
    }
    let mut seen = BTreeSet::new();
    while let Some(n) = todo.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(d) = installed.get(&n) {
            todo.extend(d.deps.iter().filter(|x| installed.contains_key(*x) && !seen.contains(*x)).cloned());
        }
    }
    seen
}

/// Why a package needn't come along, by its kind.
fn kind_of(name: &str) -> Option<&'static str> {
    let lib = name.starts_with("lib") && !["-bin", "-utils", "-tools", "-progs"].iter().any(|s| name.ends_with(s));
    if name.ends_with("-dev") || name.ends_with("-dbg") || name.ends_with("-doc") {
        Some("headers and docs for building: a project's nami file lists what it builds with")
    } else if lib {
        Some("libraries: apps on nami bring their own")
    } else if ["linux-", "grub", "shim", "efibootmgr", "intel-microcode", "amd64-microcode", "firmware-", "initramfs"].iter().any(|p| name.starts_with(p)) || name.ends_with("-firmware") {
        Some("the computer's own (kernel, firmware, boot): nami's computer has its own")
    } else if ["language-pack-", "hunspell-", "hyphen-", "mythes-", "wamerican", "wbritish"].iter().any(|p| name.starts_with(p)) || name.ends_with("-l10n") {
        Some("languages and dictionaries: language: sets the computer's")
    } else {
        None
    }
}

/// The menu entries and programs a package put on the computer.
fn deb_files(root: &Path, name: &str) -> (Vec<String>, Vec<String>) {
    let info = root.join("var/lib/dpkg/info");
    let list = ["", ":amd64", ":arm64", ":all"].iter().find_map(|a| std::fs::read_to_string(info.join(format!("{name}{a}.list"))).ok()).unwrap_or_default();
    let (mut desktops, mut programs) = (Vec::new(), Vec::new());
    for l in list.lines() {
        if let Some(d) = l.strip_prefix("/usr/share/applications/").filter(|d| d.ends_with(".desktop") && !d.contains('/')) {
            desktops.push(d.to_string());
        }
        for dir in ["/usr/bin/", "/usr/games/", "/usr/sbin/"] {
            if let Some(p) = l.strip_prefix(dir).filter(|p| !p.is_empty() && !p.contains('/')) {
                programs.push(p.to_string());
            }
        }
    }
    (desktops, programs)
}

/// The .deb apt kept in its cache, if it's there (for a recipe).
fn cached_deb(root: &Path, name: &str) -> Option<PathBuf> {
    let dir = root.join("var/cache/apt/archives");
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{name}_")) && n.ends_with(".deb")))
}

fn os_release(root: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(root.join("etc/os-release")).or_else(|_| std::fs::read_to_string(root.join("usr/lib/os-release"))).unwrap_or_default();
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.trim_matches('"').to_string())).collect()
}

/// `KEY=value` from a shell-style settings file (/etc/default/locale).
fn assignment(root: &Path, file: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(root.join(file)).ok()?;
    text.lines().filter_map(|l| l.trim().strip_prefix(&format!("{key}="))).map(|v| v.trim().trim_matches(['"', '\'']).to_string()).find(|v| !v.is_empty())
}

pub fn read_with(root: &Path, person: Person, apps: &Apps, dump: Option<std::result::Result<String, String>>) -> Result<Moved> {
    let release = os_release(root);
    let from = release.get("PRETTY_NAME").cloned().unwrap_or_else(|| "a Debian computer".into());
    let home = HomePlan { from: person.home.clone(), ..Default::default() };
    let mut m = Moved::new(from, person, home);
    let (installed, auto) = dpkg(root);
    let base = base(root, &installed);

    // The desktop: namiOS's, in place of whichever it had (namios lists
    // niri, its login screen and desktop, desktop lists base: packages
    // bring what they list); none, none.
    let desktops = [("gnome-shell", "GNOME"), ("plasma-desktop", "Plasma"), ("xfce4", "Xfce"), ("cinnamon", "Cinnamon"), ("mate-desktop", "MATE"), ("lxqt", "LXQt")];
    if let Some((_, was)) = desktops.iter().find(|(p, _)| installed.contains_key(*p)) {
        m.system.include("namios");
        m.brought.push(format!("the desktop: namiOS's, in place of {was} (namios, copied from nami's examples)"));
    } else {
        m.note("there was no desktop on the old computer, so the new one has none either (a text console); for namiOS's, `nami new <the computer's folder> --from namios` copies it next to it: then list `- namios` in system.nami's packages:");
        m.system.include("base");
    }
    if installed.contains_key("ubuntu-wsl") || root.join("etc/wsl.conf").is_file() {
        m.note("this is WSL: the apps you use on Windows are on its side; `nami import /mnt/c` brings those (and moves your Windows folders)");
    }

    // Apps from apt.
    let mut kinds: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let (mut apt, mut own) = (Vec::new(), 0);
    for name in installed.keys().filter(|n| !auto.contains(*n) && !base.contains(*n)) {
        let (desktops, programs) = deb_files(root, name);
        let matched = apps.deb(name, &desktops, &programs);
        if matched == Match::Unknown {
            if let Some(why) = kind_of(name) {
                kinds.entry(why).or_default().push(name.clone());
                continue;
            }
        }
        match matched {
            Match::Pkgs(p) => {
                for a in &p {
                    m.system.package(a, &format!("apt: {name}"));
                }
                apt.push(if p.len() == 1 && p[0] == *name { name.clone() } else { format!("{name} ({})", p.join(" ")) });
            }
            Match::Own => own += 1,
            Match::No(why) => m.system.skip(format!("apt package {name}"), why),
            Match::Unknown => m.unmatched_debs.push((name.clone(), cached_deb(root, name))),
        }
    }
    for (why, names) in kinds {
        let shown = if names.len() > 6 { format!("{}, and {} more", names[..6].join(", "), names.len() - 6) } else { names.join(", ") };
        m.system.skip(format!("{} apt package{}: {shown}", names.len(), if names.len() == 1 { "" } else { "s" }), why);
    }
    m.brought("apps you installed with apt", &apt);
    if own > 0 {
        m.note(if own == 1 { "1 apt package you installed is part of the system on nami (nothing to bring)".to_string() } else { format!("{own} apt packages you installed are part of the system on nami (nothing to bring)") });
    }

    snaps_and_flatpaks(root, apps, &mut m);
    computer(root, &mut m);
    wifi(root, &mut m);
    if let Some(d) = dump {
        match d {
            Ok(text) => gnome(&text, apps, &mut m),
            Err(why) => m.user.skip("GNOME settings", why),
        }
    }
    shell(root, &mut m);
    dotfiles(&mut m);
    toolchains(root, apps, &mut m);
    // Caches, and the files the repo makes (shell startup files: nami writes its own).
    let mut exclude: Vec<String> = [".cache", ".local/share/Trash", ".config/nami", ".local/state/nami", ".local/share/flatpak", "snap", ".var/app", ".bashrc", ".profile", ".bash_profile", ".zshrc", ".config/fish/config.fish"].iter().map(|s| s.to_string()).collect();
    exclude.extend(m.user.files.keys().cloned());
    m.home.exclude = exclude;
    Ok(m)
}

fn snaps_and_flatpaks(root: &Path, apps: &Apps, m: &mut Moved) {
    // Snaps: their files in snapd's folder are name_revision.snap.
    let mut snaps = BTreeSet::new();
    for e in std::fs::read_dir(root.join("var/lib/snapd/snaps")).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if let Some((name, _)) = n.strip_suffix(".snap").and_then(|s| s.rsplit_once('_')) {
            snaps.insert(name.to_string());
        }
    }
    let mut brought = Vec::new();
    for s in &snaps {
        // Its data, where nixpkgs' app looks for it.
        for (from, to) in [("firefox", ".mozilla"), ("thunderbird", ".thunderbird")] {
            let data = format!("snap/{from}/common/{to}");
            if s == from && m.person.home.join(&data).is_dir() && !m.person.home.join(to).exists() {
                m.home.moves.push((data, to.into()));
            }
        }
        match apps.snap(s) {
            Match::Pkgs(p) => {
                for a in &p {
                    m.system.package(a, &format!("snap: {s}"));
                }
                brought.push(if p.len() == 1 && p[0] == *s { s.clone() } else { format!("{s} ({})", p.join(" ")) });
            }
            Match::Own => {}
            Match::No(why) => m.system.skip(format!("snap {s}"), why),
            Match::Unknown => m.system.skip(format!("snap {s}"), "no app by that name in the catalog"),
        }
    }
    m.brought("snaps", &brought);
    // Flatpaks: the computer's and the person's.
    let mut ids = BTreeSet::new();
    for dir in [root.join("var/lib/flatpak/app"), m.person.home.join(".local/share/flatpak/app")] {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            ids.insert(e.file_name().to_string_lossy().into_owned());
        }
    }
    let mut brought = Vec::new();
    for id in &ids {
        match apps.flatpak(id) {
            Match::Pkgs(p) => {
                for a in &p {
                    m.system.package(a, &format!("flatpak: {id}"));
                }
                brought.push(format!("{id} ({})", p.join(" ")));
                // A flatpak keeps its settings in ~/.var/app/<id>: they go where the app looks.
                let app = format!(".var/app/{id}");
                for (from, to) in [("config", ".config"), ("data", ".local/share"), (".mozilla", ".mozilla")] {
                    if m.person.home.join(&app).join(from).is_dir() {
                        m.home.moves.push((format!("{app}/{from}"), to.into()));
                    }
                }
            }
            Match::Own => {}
            Match::No(why) => m.system.skip(format!("flatpak {id}"), why),
            // Not in the catalog: from Flathub again, the person's own (its settings
            // stay in ~/.var/app, where it looks).
            Match::Unknown if crate::flathub::valid_id(id) => {
                m.user.package(&format!("{}{id}", crate::flathub::PREFIX), &format!("flatpak: {id}"));
                brought.push(format!("{id} (from Flathub)"));
                let app = format!(".var/app/{id}");
                if m.person.home.join(&app).is_dir() {
                    m.home.moves.push((app.clone(), app));
                }
            }
            Match::Unknown => m.system.skip(format!("flatpak {id}"), "no app for it in the catalog"),
        }
    }
    m.brought("flatpaks", &brought);
}

/// The computer's name, time zone, language and keyboard, and you.
fn computer(root: &Path, m: &mut Moved) {
    let mut got = Vec::new();
    if let Some(h) = std::fs::read_to_string(root.join("etc/hostname")).ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty() && h.len() <= 63 && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')) {
        top(&mut m.system, &["hostname"], h.clone().into());
        got.push(format!("hostname {h}"));
    }
    let tz = std::fs::read_to_string(root.join("etc/timezone")).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).or_else(|| {
        let link = std::fs::read_link(root.join("etc/localtime")).ok()?;
        let s = link.to_string_lossy().into_owned();
        s.split_once("zoneinfo/").map(|(_, z)| z.to_string())
    });
    if let Some(tz) = tz {
        top(&mut m.system, &["timezone"], tz.clone().into());
        got.push(format!("time zone {tz}"));
    }
    if let Some(lang) = assignment(root, "etc/default/locale", "LANG").or_else(|| assignment(root, "etc/locale.conf", "LANG")).filter(|l| l != "C" && l != "POSIX" && l != "C.UTF-8") {
        top(&mut m.system, &["language"], lang.clone().into());
        got.push(format!("language {lang}"));
    }
    if let Some(layouts) = assignment(root, "etc/default/keyboard", "XKBLAYOUT") {
        let first = layouts.split(',').next().unwrap_or("").trim().to_string();
        if !first.is_empty() {
            top(&mut m.system, &["keyboard"], first.clone().into());
            got.push(format!("keyboard {first}"));
        }
    }
    let p = m.person.clone();
    got.push(format!("you ({}{})", p.login, if p.admin { ", an admin" } else { "" }));
    // Groups that give access to something (serial ports, docker): set up with what needs them.
    let usual = ["adm", "cdrom", "sudo", "admin", "wheel", "dip", "plugdev", "lpadmin", "sambashare", "lxd", "users", "audio", "video", "netdev", "input", "render", "systemd-journal", "staff", "floppy", "tape", "operator"];
    // One that's a nami file's people (docker's): its `users:`, if it comes along.
    for g in p.groups.iter().filter(|g| crate::import::feature_group(g).is_some()) {
        m.system.group_members.push((g.clone(), p.login.clone(), format!("group {g}")));
    }
    let extra: Vec<&str> = p.groups.iter().map(String::as_str).filter(|g| *g != p.login && !usual.contains(g) && crate::import::feature_group(g).is_none()).collect();
    if !extra.is_empty() {
        m.note(format!("you were in the group{} {}: on nami, a package that needs one brings it (`groups` in nami options adds people to one)", if extra.len() == 1 { "" } else { "s" }, extra.join(", ")));
    }
    m.brought("the computer", &got);
    docker_settings(root, m);
}

/// Docker's settings (/etc/docker/daemon.json), as they are, if Docker
/// comes along (layers/docker.nami).
fn docker_settings(root: &Path, m: &mut Moved) {
    let Ok(text) = std::fs::read_to_string(root.join("etc/docker/daemon.json")) else { return };
    let settings = match serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|j| serde_yaml::to_value(j).ok()) {
        Some(Value::Mapping(s)) => s,
        _ => return m.system.skip("/etc/docker/daemon.json", "Docker's settings, but not JSON nami could read"),
    };
    let file = Mapping::from_iter([("content".into(), Value::Mapping(settings)), ("restart".into(), Value::String("docker.service".into()))]);
    m.system.if_included.push(("docker".into(), vec!["files".into(), "/etc/docker/daemon.json".into()], Value::Mapping(file)));
}

/// NetworkManager's Wi-Fi networks, passwords sealed.
fn wifi(root: &Path, m: &mut Moved) {
    let dir = root.join("etc/NetworkManager/system-connections");
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            m.system.skip("Wi-Fi networks", "NetworkManager keeps them for root alone: run `sudo nami import /` to bring them (their passwords are sealed, never written plain)");
            return;
        }
        Err(_) => return,
    };
    let mut names = Vec::new();
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    files.sort();
    for f in files {
        let text = match std::fs::read_to_string(&f) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                m.system.skip("Wi-Fi networks", "NetworkManager keeps them for root alone: run `sudo nami import /` to bring them (their passwords are sealed, never written plain)");
                return;
            }
            Err(_) => continue,
        };
        match keyfile(&text) {
            Ok(Some((id, settings))) => {
                add_wifi(&mut m.system, &id, settings);
                names.push(id);
            }
            Ok(None) => {}
            Err((id, why)) => m.system.skip(format!("Wi-Fi network {id}"), why),
        }
    }
    m.brought("Wi-Fi networks (passwords sealed)", &names);
}

/// A NetworkManager keyfile as a `wifi:` entry, if it's a Wi-Fi network
/// (Ok(None) for other connections); Err((id, why)) for one nami can't bring.
pub fn keyfile(text: &str) -> std::result::Result<Option<(String, Mapping)>, (String, String)> {
    let mut sections: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((s.to_string(), BTreeMap::new()));
            continue;
        }
        let (Some((_, m)), Some((k, v))) = (sections.last_mut(), line.split_once('=')) else { continue };
        m.insert(k.trim().into(), v.trim().into());
    }
    let get = |s: &[&str], k: &str| sections.iter().find(|(n, _)| s.contains(&n.as_str())).and_then(|(_, m)| m.get(k)).filter(|v| !v.is_empty()).cloned();
    let ty = get(&["connection"], "type").unwrap_or_default();
    if ty != "wifi" && ty != "802-11-wireless" {
        return Ok(None);
    }
    let id = get(&["connection"], "id").or_else(|| get(&["wifi", "802-11-wireless"], "ssid")).unwrap_or_else(|| "wifi".into());
    let sec = ["wifi-security", "802-11-wireless-security"];
    let mut w = Wifi {
        ssid: get(&["wifi", "802-11-wireless"], "ssid"),
        hidden: get(&["wifi", "802-11-wireless"], "hidden").as_deref() == Some("true"),
        autoconnect: get(&["connection"], "autoconnect").as_deref() != Some("false"),
        ..Default::default()
    };
    // Certificates are files on the old computer, not settings.
    if ["ca-cert", "client-cert", "private-key", "phase2-ca-cert", "phase2-client-cert"].iter().any(|k| get(&["802-1x"], k).is_some()) {
        return Err((id, "a network that logs in with certificate files: set it up again in Settings".into()));
    }
    match get(&sec, "key-mgmt").as_deref() {
        None | Some("none") if get(&sec, "wep-key0").is_none() => w.security = Some("open"),
        Some("wpa-psk") => w.security = Some("wpa-psk"),
        Some("sae") => w.security = Some("sae"),
        Some("wpa-eap") | Some("wpa-eap-suite-b-192") => {
            let eap = get(&["802-1x"], "eap").unwrap_or_default().trim_end_matches(';').to_string();
            if eap != "peap" && eap != "ttls" {
                return Err((id, format!("its log-in ({eap}) isn't one nami sets up (peap, ttls): set it up again in Settings")));
            }
            w.security = Some("enterprise");
            w.identity = get(&["802-1x"], "identity");
            w.eap = Some(eap);
            w.phase2 = get(&["802-1x"], "phase2-auth");
            if w.phase2.as_deref().is_some_and(|p| p != "mschapv2" && p != "pap") {
                return Err((id, "its inner log-in isn't one nami sets up (mschapv2, pap): set it up again in Settings".into()));
            }
            w.password = get(&["802-1x"], "password");
        }
        other => return Err((id, format!("its security ({}) isn't one nami sets up: set it up again in Settings", other.unwrap_or("wep")))),
    }
    if w.security != Some("enterprise") {
        w.password = get(&sec, "psk");
    }
    let entry = w.entry(&id);
    Ok(Some((id, entry)))
}

/// GNOME's settings the person chose, with the favourite apps as namiOS's
/// dock's.
fn gnome(dump: &str, apps: &Apps, m: &mut Moved) {
    let mut picked = dconf::pick(dump);
    // Favourite apps: the dock's, by their menu entries' names, as nami
    // names them (snaps' and flatpaks' are named differently on nami).
    let favourites = picked.settings.get_mut("org.gnome.shell").and_then(|k| k.remove("favorite-apps"));
    let mut pinned = Vec::new();
    for d in favourites.as_ref().and_then(Value::as_sequence).into_iter().flatten().filter_map(Value::as_str) {
        if d.starts_with("snap-store_") {
            continue;
        }
        // firefox_firefox.desktop -> firefox.desktop
        let entry = match d.split_once('_') {
            Some((_, rest)) => rest.to_string(),
            None => match apps.flatpak(d.trim_end_matches(".desktop")) {
                Match::Pkgs(p) if apps.desktop(d).is_none() => p.first().map(|a| format!("{a}.desktop")).unwrap_or_else(|| d.to_string()),
                _ => d.to_string(),
            },
        };
        pinned.push(Value::String(entry.trim_end_matches(".desktop").to_string()));
    }
    if !pinned.is_empty() {
        m.brought.push(format!("your favourite apps, in the dock ({})", pinned.len()));
        top(&mut m.user, &["namios", "dock", "pinned"], Value::Sequence(pinned));
    }
    // A wallpaper of the person's own comes with their files; Ubuntu's doesn't.
    let home = format!("file://{}", m.person.home.display());
    for schema in ["org.gnome.desktop.background", "org.gnome.desktop.screensaver"] {
        if let Some(keys) = picked.settings.get_mut(schema) {
            keys.retain(|k, v| !k.starts_with("picture-uri") || v.as_str().is_some_and(|u| u.starts_with(&home) || u.starts_with("file:///home/")));
        }
    }
    picked.settings.retain(|_, k| !k.is_empty());
    let n: usize = picked.settings.values().map(|k| k.len()).sum();
    let mut about = Vec::new();
    let has = |s: &str, k: &str| picked.settings.get(s).is_some_and(|m| m.contains_key(k));
    for (s, k, what) in [
        ("org.gnome.desktop.interface", "color-scheme", "dark or light"),
        ("org.gnome.desktop.input-sources", "sources", "keyboard layouts"),
        ("org.gnome.settings-daemon.plugins.color", "night-light-enabled", "night light"),
        ("org.gnome.desktop.interface", "gtk-theme", "theme"),
    ] {
        if has(s, k) {
            about.push(what.to_string());
        }
    }
    if picked.settings.contains_key("org.gnome.desktop.peripherals.touchpad") {
        about.push("touchpad".into());
    }
    if picked.settings.keys().any(|k| k.contains("keybinding")) {
        about.push("key bindings".into());
    }
    m.user.settings.extend(picked.settings);
    if n > 0 {
        m.brought.push(format!("GNOME settings ({n}): {}", if about.is_empty() { "the ones you changed".into() } else { about.join(", ") }));
    }
    if picked.left_out > 0 {
        m.note(format!("{} other GNOME settings stayed behind: app state, like window sizes and recent files", picked.left_out));
    }
}

/// The login shell, and what the person added to their shell's startup.
fn shell(root: &Path, m: &mut Moved) {
    let p = m.person.clone();
    let sh = p.shell.as_deref().and_then(|s| s.rsplit('/').next()).unwrap_or("bash").to_string();
    if sh == "zsh" || sh == "fish" {
        m.user.package(&sh, "your login shell");
        m.user.login_shell = Some(sh.clone());
    }
    let skel = |f: &str| std::fs::read_to_string(root.join("etc/skel").join(f)).unwrap_or_default();
    let mut lines_from = |file: &str, sh: &str, skel_text: String| {
        let Ok(text) = std::fs::read_to_string(p.home.join(file)) else { return };
        let skel_lines: BTreeSet<&str> = skel_text.lines().map(str::trim).collect();
        let mut kept = Vec::new();
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') || skel_lines.contains(t) {
                continue;
            }
            // `export NAME=value` and `alias x='y'`: nami's own.
            if let Some((name, value)) = t.strip_prefix("export ").and_then(|r| r.split_once('=')) {
                let value = value.trim().trim_matches(['"', '\'']);
                if super::valid_env_name(name) && !value.contains(['$', '`', ' ']) && sh != "fish" {
                    if secret_name(name) || secret_text(t) {
                        // Sealed before it's written.
                        m.user.top.push((vec!["env".into(), name.into()], secret(value)));
                    } else {
                        m.user.env.insert(name.to_string(), value.to_string());
                    }
                    continue;
                }
            }
            if let Some((name, value)) = t.strip_prefix("alias ").and_then(|r| r.split_once('=')) {
                let value = value.trim();
                let unq = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')).or_else(|| value.strip_prefix('"').and_then(|v| v.strip_suffix('"')));
                if let Some(v) = unq.filter(|_| !secret_text(t)) {
                    m.user.aliases.insert(name.trim().to_string(), v.to_string());
                    continue;
                }
            }
            if secret_text(t) {
                m.user.skip(format!("a line in ~/{file}"), "it looks like it holds a secret: add it again with `env: { NAME: !secret ... }`");
                continue;
            }
            kept.push(line.to_string());
        }
        if !kept.is_empty() {
            let entry = m.user.shell.entry(sh.to_string()).or_default();
            *entry += &format!("# from ~/{file}\n{}\n", kept.join("\n"));
        }
    };
    lines_from(".profile", "bash", skel(".profile"));
    lines_from(".bashrc", "bash", skel(".bashrc"));
    lines_from(".zshrc", "zsh", skel(".zshrc"));
    lines_from(".config/fish/config.fish", "fish", String::new());
    let mut what = Vec::new();
    if let Some(l) = &m.user.login_shell {
        what.push(format!("login shell {l}"));
    }
    let vars = m.user.env.len() + m.user.top.iter().filter(|(p, _)| p.get(1).map(String::as_str) == Some("env")).count();
    for (one, many, n) in [("variable", "variables", vars), ("alias", "aliases", m.user.aliases.len())] {
        if n > 0 {
            what.push(format!("{n} {}", if n == 1 { one } else { many }));
        }
    }
    what.extend(m.user.shell.keys().map(|s| format!("{s} startup lines")));
    m.brought("your shell", &what);
}

/// Small settings files worth keeping in the repo; a private key never.
const DOTFILES: &[&str] = &[".gitconfig", ".config/git/config", ".config/git/ignore", ".ssh/config", ".vimrc", ".tmux.conf", ".config/tmux/tmux.conf", ".inputrc", ".config/starship.toml", ".npmrc", ".editorconfig", ".config/nvim/init.vim", ".hgrc", ".wgetrc", ".curlrc"];

fn dotfiles(m: &mut Moved) {
    let mut got = Vec::new();
    for rel in DOTFILES {
        let path = m.person.home.join(rel);
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if !meta.is_file() || meta.len() > 64 * 1024 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if text.contains("PRIVATE KEY") {
            m.user.skip(format!("~/{rel}"), "it holds a private key, which never goes in a repo (it moves with your files)");
            continue;
        }
        let value = if secret_text(&text) {
            // The whole file, sealed: written for you alone.
            Value::Mapping(Mapping::from_iter([("content".into(), secret(&text)), ("mode".into(), "0600".into())]))
        } else {
            // Indented with spaces, so it reads as a block in the nami file
            // (these formats don't mind which).
            let lines: Vec<String> = text.lines().map(|l| {
                let tabs = l.len() - l.trim_start_matches('\t').len();
                format!("{}{}", "    ".repeat(tabs), &l[tabs..])
            }).collect();
            Value::String(lines.join("\n") + if text.ends_with('\n') { "\n" } else { "" })
        };
        m.user.files.insert(rel.to_string(), value);
        got.push(rel.to_string());
    }
    if m.person.home.join(".ssh").is_dir() {
        m.note("your SSH keys aren't in the repo (never): they move with your files (`--home`)");
    }
    m.brought("dotfiles", &got);
}

/// Language toolchains installed in the home (rustup, nvm, pyenv...).
fn toolchains(root: &Path, apps: &Apps, m: &mut Moved) {
    let h = m.person.home.clone();
    let names_in = |p: PathBuf| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect();
        v.sort();
        v
    };
    let has = |a: &str| apps.catalog.as_ref().is_none_or(|c| c.has(a));
    let mut got = Vec::new();
    if h.join(".rustup").is_dir() {
        m.user.package("rustup", "rustup in your home");
        let tc = names_in(h.join(".rustup/toolchains"));
        got.push("rustup".into());
        if !tc.is_empty() {
            m.note(format!("rustup's toolchains come with your files (~/.rustup): {}", tc.join(", ")));
        }
    }
    // Programs you installed with `cargo install`.
    if let Ok(text) = std::fs::read_to_string(h.join(".cargo/.crates.toml")) {
        let crates: Vec<String> = text.lines().filter_map(|l| l.trim().strip_prefix('"')).filter_map(|l| l.split_whitespace().next()).map(String::from).collect();
        let (mut found, mut not) = (Vec::new(), Vec::new());
        for c in crates {
            match apps.catalog.as_ref().and_then(|cat| cat.resolve(&c).or_else(|| cat.attr_for_pname(&c))) {
                Some(a) => {
                    m.user.package(&a, &format!("cargo install {c}"));
                    found.push(a);
                }
                None => not.push(c),
            }
        }
        got.extend(found.iter().map(|a| format!("{a} (was cargo install)")));
        if !not.is_empty() {
            m.note(format!("cargo install'd, not in the catalog (they come with ~/.cargo): {}", not.join(", ")));
        }
    }
    if h.join(".nvm").is_dir() {
        let versions = names_in(h.join(".nvm/versions/node"));
        match versions.last().and_then(|v| v.trim_start_matches('v').split('.').next().map(String::from)) {
            Some(major) => {
                let pkg = Some(format!("nodejs_{major}")).filter(|p| has(p)).unwrap_or_else(|| "nodejs".into());
                m.user.package(&pkg, "nvm in your home");
                got.push(format!("{pkg} (nvm had {})", versions.join(", ")));
                m.note("nvm itself comes with your files (~/.nvm); a project's nami file can pin its own node");
            }
            None => m.note("nvm is in your home, with no node installed: nothing to bring (it comes with your files)"),
        }
    }
    if h.join(".pyenv").is_dir() {
        let versions = names_in(h.join(".pyenv/versions"));
        for v in &versions {
            let mut parts = v.split('.');
            if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                let pkg = format!("python{a}{b}");
                if a.chars().all(|c| c.is_ascii_digit()) && has(&pkg) {
                    m.user.package(&pkg, "pyenv in your home");
                    got.push(format!("{pkg} (pyenv's {v})"));
                }
            }
        }
    }
    for (dir, pkg) in [(".deno", "deno"), (".bun", "bun"), ("go", "go"), (".sdkman", "jdk")] {
        if h.join(dir).is_dir() && has(pkg) {
            m.user.package(pkg, &format!("~/{dir}"));
            got.push(pkg.to_string());
        }
    }
    if root.join("usr/local/go/bin/go").is_file() {
        m.user.package("go", "/usr/local/go");
        got.push("go".into());
    }
    // pipx's apps.
    for app in names_in(h.join(".local/share/pipx/venvs")) {
        match apps.catalog.as_ref().and_then(|c| c.resolve(&app)) {
            Some(a) => {
                m.user.package(&a, "pipx");
                got.push(format!("{a} (was pipx)"));
            }
            None => m.user.skip(format!("pipx app {app}"), "not in the catalog"),
        }
    }
    got.dedup();
    m.brought("toolchains", &got);
    // Programs put in /usr/local/bin by hand.
    let local = names_in(root.join("usr/local/bin"));
    if !local.is_empty() {
        m.system.skip(format!("/usr/local/bin: {}", local.join(", ")), "installed by hand, not by a package: look for each with `nami search`");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/import/computer/ubuntu")
    }

    #[test]
    fn keyfiles() {
        let (id, s) = keyfile("[connection]\nid=Home\ntype=wifi\ninterface-name=wlp2s0\n[wifi]\nmode=infrastructure\nssid=Home\nmac-address=AA:BB\n[wifi-security]\nkey-mgmt=wpa-psk\npsk=hunter22\n[ipv4]\nmethod=auto\n").unwrap().unwrap();
        assert_eq!(id, "Home");
        let y = serde_yaml::to_string(&s).unwrap();
        assert_eq!(y, "password: !secret hunter22\n");
        let (_, s) = keyfile("[connection]\nid=Uni\ntype=wifi\nautoconnect=false\n[wifi]\nssid=eduroam\nhidden=true\n[wifi-security]\nkey-mgmt=wpa-eap\n[802-1x]\neap=ttls;\nidentity=alice@uni.example\npassword=pw\nphase2-auth=pap\n").unwrap().unwrap();
        let y = serde_yaml::to_string(&s).unwrap();
        for want in ["ssid: eduroam", "security: enterprise", "identity: alice@uni.example", "eap: ttls", "phase2: pap", "password: !secret pw", "hidden: true", "autoconnect: false"] {
            assert!(y.contains(want), "{want} in {y}");
        }
        let (_, s) = keyfile("[connection]\nid=Cafe\ntype=wifi\n[wifi]\nssid=Cafe\n").unwrap().unwrap();
        assert_eq!(serde_yaml::to_string(&s).unwrap(), "security: open\n");
        assert!(keyfile("[connection]\nid=Wired\ntype=ethernet\n").unwrap().is_none());
        assert!(keyfile("[connection]\nid=Corp\ntype=wifi\n[802-1x]\neap=tls\nca-cert=/etc/x.pem\n").is_err());
    }

    #[test]
    fn an_ubuntu_computer() {
        let root = fixture().join("root");
        let names = ["btop", "git", "vscode", "google-chrome", "fira-code", "qemu", "ripgrep", "fd", "gimp", "spotify", "gnome-boxes", "obsidian", "rustup", "nodejs", "nodejs_22", "python312", "zsh", "bat", "gnomeExtensions.caffeine", "firefox"];
        let apps = Apps::with_names(&names).with_file("gimp.desktop", "gimp").with_file("rg", "ripgrep");
        let p = person(&root, Some("alice")).unwrap();
        assert!(p.admin && p.groups.contains(&"dialout".to_string()) && p.name.as_deref() == Some("Alice Liddell"));
        assert!(person(&root, None).is_err(), "two people: say which");
        let dump = std::fs::read_to_string(fixture().join("dconf.txt")).unwrap();
        let mut m = read_with(&root, p, &apps, Some(Ok(dump))).unwrap();
        let pk: Vec<&str> = m.system.packages.iter().map(|(a, _)| a.as_str()).collect();
        for want in ["btop", "vscode", "google-chrome", "fira-code", "qemu", "ripgrep", "fd", "gimp", "spotify", "gnome-boxes", "obsidian"] {
            assert!(pk.contains(&want), "{want} in {pk:?}");
        }
        // What the system brought stays out: bash, gnome-shell (ubuntu-desktop's).
        assert!(!pk.contains(&"bash") && !pk.contains(&"gnome-shell") && !pk.contains(&"nautilus"), "{pk:?}");
        assert_eq!(m.unmatched_debs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["acme-vpn"]);
        let skipped: Vec<String> = m.system.unconverted.iter().map(|u| u.what.clone()).collect();
        assert!(skipped.iter().any(|s| s.contains("libssl-dev")) && skipped.iter().any(|s| s.contains("linux-generic")), "{skipped:?}");
        assert!(skipped.iter().any(|s| s == "snap acme-notes"), "{skipped:?}");
        // Wi-Fi: passwords as secrets, the open one in /etc for root alone, the certificate one listed.
        let wifi: BTreeMap<String, Value> = m.system.top.iter().filter(|(p, _)| p[0] == "wifi").map(|(p, v)| (p[1].clone(), v.clone())).collect();
        assert!(serde_yaml::to_string(&wifi["Home"]).unwrap().contains("password: !secret"));
        assert!(wifi.contains_key("Cafe Wifi") && wifi.contains_key("Campus"));
        assert!(m.system.etc_files.is_empty());
        assert!(skipped.iter().any(|s| s == "Wi-Fi network Corp"));
        assert!(!wifi.contains_key("Wired connection 1"));
        let tops: BTreeMap<String, Value> = m.system.top.iter().map(|(p, v)| (p.join("."), v.clone())).collect();
        assert_eq!(m.system.includes, ["namios"]);
        assert_eq!(tops["hostname"], Value::String("oldlaptop".into()));
        assert_eq!(tops["timezone"], Value::String("Europe/Copenhagen".into()));
        assert_eq!(tops["language"], Value::String("da_DK.UTF-8".into()));
        assert_eq!(tops["keyboard"], Value::String("dk".into()));
        // The person: settings, shell, dotfiles, toolchains.
        let u = &m.user;
        assert_eq!(u.settings["org.gnome.desktop.interface"]["color-scheme"], Value::String("prefer-dark".into()));
        // Favourite apps: the dock's, as nami names them; GNOME's own
        // extensions stay behind.
        let pinned = u.top.iter().find(|(p, _)| p.join(".") == "namios.dock.pinned").map(|(_, v)| v.clone());
        assert_eq!(pinned, Some(serde_yaml::from_str::<Value>("[firefox, org.gnome.Nautilus, code, spotify]").unwrap()));
        assert!(!u.settings.contains_key("org.gnome.shell") && !u.packages.iter().any(|(a, _)| a.starts_with("gnomeExtensions.")));
        assert_eq!(u.env.get("EDITOR").map(String::as_str), Some("vim"));
        assert!(!u.env.contains_key("GITHUB_TOKEN"));
        assert!(u.top.iter().any(|(p, v)| p.join(".") == "env.GITHUB_TOKEN" && matches!(v, Value::Tagged(_))));
        assert_eq!(u.aliases.get("ll").map(String::as_str), Some("ls -alF --color"));
        assert!(u.shell["bash"].contains("starship init bash") && !u.shell["bash"].contains("HISTCONTROL"), "{}", u.shell["bash"]);
        assert!(u.files.contains_key(".gitconfig") && u.files.contains_key(".ssh/config"));
        assert!(!u.files.keys().any(|k| k.contains("id_ed25519")));
        assert!(matches!(&u.files[".npmrc"], Value::Mapping(_)), "a file with a token is sealed whole");
        for want in ["rustup", "nodejs_22", "python312", "ripgrep"] {
            assert!(u.packages.iter().any(|(a, _)| a == want), "{want}");
        }
        // Your files: caches and what the repo makes stay behind; Firefox's profile moves.
        assert!(m.home.exclude.contains(&".cache".into()) && m.home.exclude.contains(&".gitconfig".into()) && m.home.exclude.contains(&".bashrc".into()));
        assert!(m.home.moves.contains(&("snap/firefox/common/.mozilla".into(), ".mozilla".into())));
        // Docker (docker.io, docker-compose-v2; alice in the docker group,
        // a daemon.json): its nami file, with her in its group and the
        // settings, as `nami import` finishes it.
        let pk: Vec<&str> = m.system.packages.iter().map(|(a, _)| a.as_str()).collect();
        assert!(pk.contains(&"docker") && pk.contains(&"docker-compose") && !pk.contains(&"containerd"), "{pk:?}");
        assert!(!m.notes.iter().any(|n| n.contains("docker")), "{:?}", m.notes);
        top(&mut m.system, &["users", "alice"], Value::Mapping(Mapping::new()));
        crate::import::features(&mut m.system, true);
        let pk: Vec<&str> = m.system.packages.iter().map(|(a, _)| a.as_str()).collect();
        assert!(!pk.iter().any(|p| p.starts_with("docker")), "the docker example brings them: {pk:?}");
        assert_eq!(m.system.includes, ["namios", "docker"]);
        let tops: BTreeMap<String, Value> = m.system.top.iter().map(|(p, v)| (p.join("."), v.clone())).collect();
        let y = |s: &str| -> Value { serde_yaml::from_str(s).unwrap() };
        assert_eq!(tops["users.alice"], y("{ groups: [docker] }"));
        assert_eq!(tops["files./etc/docker/daemon.json"], y("{ content: { log-driver: local, registry-mirrors: [https://mirror.example.org], live-restore: true }, restart: docker.service }"));
        // The file it writes checks clean.
        let (text, added, _) = crate::import::merge("", &m.system, crate::import::Tier::System, "Ubuntu").unwrap();
        assert!(added.contains(&"package docker (from nami's examples)".to_string()) && text.contains("packages:\n  - namios\n  - docker\n") && !text.contains("from:"), "{added:?}\n{text}");
        // With the examples it lists copied next to it. (Its secrets are sealed as it writes the file.)
        let dir = std::env::temp_dir().join(format!("nami-ubuntu-import-{}/oldlaptop", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::new::copy_packages(&dir, &m.system.includes).unwrap();
        let errors: Vec<String> = crate::config::check_tier(&text, &dir, None, crate::config::Tier::System).into_iter().filter(|d| d.severity == crate::config::Severity::Error && !d.message.contains("isn't sealed yet")).map(|d| d.message).collect();
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        // Docker not installed, someone in its group all the same: listed, nothing set.
        let mut f = crate::import::Found::default();
        f.group_members.push(("docker".into(), "alice".into(), "group docker".into()));
        f.if_included.push(("docker".into(), vec!["docker".into(), "dns".into()], y("[1.1.1.1]")));
        crate::import::features(&mut f, true);
        assert!(f.top.is_empty() && f.unconverted[0].why.contains("`- docker` in packages: (`nami new <the computer's folder> --from docker` copies it from nami's examples) and `groups: [docker]` under users.alice"), "{:?}", f.unconverted);
    }
}
