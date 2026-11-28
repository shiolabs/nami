//! A computer's own keys in system.nami (its name, people, desktops, time zone...):
//! what they are, and what's wrong with them. Every system.nami has them, on any
//! distribution: read.rs gathers them into `Config.os`. namiOS makes the computer from
//! them; elsewhere `nami check` and `nami apply` say which only work on namiOS.

use std::collections::BTreeMap;

use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Deserializer};

use crate::settings::Settings;

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Os {
    /// The desktops to pick from on the login screen; the first is the default.
    #[serde(default, deserialize_with = "one_or_many")]
    pub desktop: Vec<String>,
    /// The login screen (display manager) by its service's name (`gdm`,
    /// `greetd`): layers/login-*.nami name theirs; with several, the system
    /// file's own choice, else the first desktop's.
    #[serde(default, rename = "login-screen")]
    pub login_screen: Option<String>,
    pub hostname: Option<String>,
    pub timezone: Option<String>,
    /// A locale, like en_US.UTF-8 or da_DK.UTF-8.
    pub language: Option<String>,
    /// A keyboard layout, like us or dk.
    pub keyboard: Option<String>,
    #[serde(default)]
    pub users: BTreeMap<String, User>,
    /// Desktop settings everyone starts with (they can change them).
    #[serde(default)]
    pub settings: Option<Settings>,
    /// What the computer is made of (nami's own files list theirs here: a
    /// file that includes them on an ordinary computer doesn't get them).
    #[serde(default)]
    pub packages: Vec<String>,
    /// Packages the included files bring that you don't want.
    #[serde(default)]
    pub without: Vec<String>,
    /// Files for the computer, like `files:` (which wins where both say one).
    #[serde(default)]
    pub files: serde_yaml::Mapping,
    /// Services of the package folders' own (`run:`), like `services:`.
    #[serde(default)]
    pub services: serde_yaml::Mapping,
    /// Services the packages bring, started with the computer (as
    /// `systemctl enable` would: where their [Install] says).
    #[serde(default)]
    pub start: Vec<String>,
    /// Each person's services the packages bring, started at their login.
    #[serde(default, rename = "start-per-person")]
    pub start_per_person: Vec<String>,
    /// Environment variables for everyone (logins and the desktop).
    #[serde(default, deserialize_with = "crate::yaml_text::text_map")]
    pub environment: BTreeMap<String, String>,
    /// Programs that act as root, or need a copy of their own, for whoever
    /// runs them: "MODE path" in the packages (`4755 bin/sudo`), copied to
    /// /run/wrappers/bin.
    #[serde(default)]
    pub wrappers: Vec<String>,
    /// Lines for the login rules (PAM) of every way to log in: `auth ...`,
    /// `password ...`, `session ...`, each after nami's own of that kind.
    #[serde(default)]
    pub login: Vec<String>,
    /// Login rules (PAM) for the programs a package brings that check who
    /// you are: `login` (the rules for logging in), `su` (root goes ahead,
    /// anyone else gives their password), or the rules themselves.
    #[serde(default, rename = "login-services")]
    pub login_services: BTreeMap<String, String>,
    /// Groups the people on the computer join: a group's name, and
    /// `everyone` or `admins` (the group comes from a package's sysusers).
    #[serde(default)]
    pub groups: BTreeMap<String, String>,
    /// Ways to look up computers' names, before DNS (`mdns_minimal
    /// [NOTFOUND=return]`); their modules are found in the packages.
    #[serde(default, rename = "name-lookup")]
    pub name_lookup: Vec<String>,
    /// systemd units a package needs and doesn't bring (nixpkgs leaves some
    /// to NixOS): a name like `avahi-daemon.service`, and the unit's text;
    /// or a drop-in that changes one (`gdm.service.d/nami.conf`). With
    /// `user/` before the name, each person's own (`user/notes.path`).
    #[serde(default)]
    pub units: BTreeMap<String, String>,
    /// Services that mustn't run (systemd's own network manager, next to
    /// NetworkManager).
    #[serde(default)]
    pub mask: Vec<String>,
    /// Files worked out from what's installed, when the system is put
    /// together: a path, and a shell command that writes it (a file or a
    /// folder) to $out ($packages: every package's folder; tools of
    /// coreutils, grep, sed, find, and `build-tools`).
    #[serde(default)]
    pub build: BTreeMap<String, String>,
    /// Packages whose programs `build` uses, fetched for that and not put
    /// on the computer (`glib.dev`: glib-compile-schemas).
    #[serde(default, rename = "build-tools")]
    pub build_tools: Vec<String>,
    /// Archives the kernel reads before the initrd (the processor's
    /// microcode: `intel-ucode.img`), a path in one of the packages.
    #[serde(default, rename = "early-boot")]
    pub early_boot: Vec<String>,
    /// Programs the initrd needs before the root is there (`bin/cryptsetup`,
    /// to unlock an encrypted disk), a path in one of the packages; their
    /// libraries come along.
    #[serde(default, rename = "initrd-programs")]
    pub initrd_programs: Vec<String>,
    /// Kernel modules the initrd loads, next to its own (`dm_crypt`).
    #[serde(default, rename = "initrd-modules")]
    pub initrd_modules: Vec<String>,
    /// The kernel, by its nixpkgs name (`linux_zen`); nixpkgs' default
    /// (`linux`) without. Packages built for a kernel (`linuxPackages.x`)
    /// are then that one's.
    #[serde(default)]
    pub kernel: Option<String>,
    /// Packages of kernel modules built for the system's kernel (a driver
    /// that isn't part of it): they join the kernel's own in /lib/modules.
    #[serde(default, rename = "kernel-modules")]
    pub kernel_modules: Vec<String>,
    /// Packages of firmware for the kernel (`linux-firmware`), in /lib/firmware.
    #[serde(default)]
    pub firmware: Vec<String>,
    /// The graphics drivers (`mesa`): their libraries go in one place,
    /// /run/opengl-driver, where OpenGL, EGL, Vulkan and GBM look for them.
    #[serde(default)]
    pub graphics: Vec<String>,
    /// Their 32-bit builds (`pkgsi686Linux.mesa`), in /run/opengl-driver-32,
    /// for 32-bit programs (Steam's games).
    #[serde(default, rename = "graphics-32bit")]
    pub graphics_32bit: Vec<String>,
    /// Packages whose libraries programs built for other distributions find
    /// (through nix-ld, at /lib64): /run/current-system/sw/share/nix-ld/lib.
    #[serde(default)]
    pub libraries: Vec<String>,
    /// More of the kernel's command line (`pcie_aspm=off`), in every boot entry.
    #[serde(default, rename = "kernel-options")]
    pub kernel_options: Vec<String>,
    /// How to tell it works, on the computer: a name, and a command whose
    /// output has `expect` in it (a regular expression). tests/usb.sh runs them.
    #[serde(default)]
    pub checks: BTreeMap<String, Check>,
    /// Secure Boot: where shim (signed by Microsoft) and its MokManager
    /// come from, a file in a Debian package each (nami's secure-boot example).
    #[serde(default, rename = "secure-boot")]
    pub secure_boot: Option<SecureBootFiles>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub run: String,
    pub expect: String,
}

/// os.secure-boot: where shim and MokManager come from.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecureBootFiles {
    pub shim: Download,
    pub mokmanager: Download,
}

/// A file in a Debian package: the package's addresses (the first that
/// answers), its sha256, and the file's path in it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Download {
    #[serde(deserialize_with = "one_or_many")]
    pub from: Vec<String>,
    pub sha256: String,
    pub file: String,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    /// May change the computer (sudo, settings for everyone).
    #[serde(default)]
    pub admin: bool,
    /// The full name the login screen shows.
    pub name: Option<String>,
    /// A password hash (`$6$...`, from `mkpasswd`); without one, `nami usb` asks.
    pub password: Option<String>,
    /// More groups they're in (`docker`), as `usermod -aG` puts someone in.
    #[serde(default)]
    pub groups: Vec<String>,
    /// A child: no sudo, no apps of their own, and the limits below
    /// (parental controls, malcontent's: the parental-controls package).
    #[serde(default)]
    pub child: bool,
    /// When a child may use the computer, `07:00-20:00`: logging in only
    /// then, and their session ends at the end.
    pub hours: Option<String>,
    /// Apps a child can't use: off their menu, and their desktop won't
    /// start them.
    #[serde(default)]
    pub without: Vec<String>,
}

/// `07:00-20:00`: from and to, as seconds after midnight (to may be 24:00).
pub fn parse_hours(s: &str) -> Option<(u32, u32)> {
    let time = |t: &str| -> Option<u32> {
        let (h, m) = t.trim().split_once(':')?;
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        (m < 60 && (h < 24 || (h == 24 && m == 0)) && t.trim().len() == 5).then_some(h * 3600 + m * 60)
    };
    let (from, to) = s.split_once('-')?;
    let (from, to) = (time(from)?, time(to)?);
    (from < to).then_some((from, to))
}

pub(crate) fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Either::deserialize(d)? {
        Either::One(s) => vec![s],
        Either::Many(v) => v,
    })
}

/// The desktops on the computer loaded last: each package folder it lists
/// that lists a login screen (examples/packages/niri lists gdm). No
/// desktop at all is a computer with a text console, like a server.
pub fn desktops() -> Vec<String> {
    crate::config::folder_desktops()
}

/// The group of the computer's children (os.users.<them>.child): the
/// parental controls' login rules are theirs alone.
pub const CHILDREN: &str = "children";

/// What's wrong with an `os:` section, in words.
pub fn check(os: &Os) -> Vec<String> {
    let mut out = Vec::new();
    for d in &os.desktop {
        let known = desktops();
        if !known.contains(d) {
            let hint = if known.is_empty() { String::new() } else { format!(": listed here: {}", known.join(", ")) };
            out.push(format!("os.desktop: no desktop {d} (a desktop is a package folder you list that lists a login screen, gdm or namios-greeter{hint})"));
        }
    }
    for (g, who) in &os.groups {
        if !matches!(who.as_str(), "everyone" | "admins") {
            out.push(format!("os.groups.{g}: {who:?}: everyone or admins"));
        }
    }
    for (name, u) in &os.users {
        let ok = name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && name.len() <= 32
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !ok || ["root", "nobody", "nami"].contains(&name.as_str()) {
            out.push(format!("os.users.{name}: a user name is lowercase letters, digits, - and _ (and not a system account's)"));
        }
        if let Some(p) = &u.password {
            if !p.starts_with('$') {
                out.push(format!("os.users.{name}.password: give the hash (`mkpasswd`), never the password itself; or leave it out and `nami usb` asks"));
            }
        }
        if u.child && u.admin {
            out.push(format!("os.users.{name}: a child can't be an admin too (child: true or admin: true)"));
        }
        if !u.child && (u.hours.is_some() || !u.without.is_empty()) {
            let key = if u.hours.is_some() { "hours" } else { "without" };
            out.push(format!("os.users.{name}.{key}: limits are for a child (child: true)"));
        }
        if let Some(h) = u.hours.as_deref().filter(|h| parse_hours(h).is_none()) {
            out.push(format!("os.users.{name}.hours: {h:?}: from-to in 24-hour time, like 07:00-20:00"));
        }
        for g in &u.groups {
            let ok = g.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_') && g.len() <= 32 && g.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
            if !ok || g == "wheel" || g == CHILDREN {
                out.push(format!("os.users.{name}.groups: {g}: a group's name, lowercase letters, digits, - and _ (`admin: true` for wheel, `child: true` for children)"));
            }
        }
    }
    if let Some(h) = &os.hostname {
        if h.is_empty() || h.len() > 63 || !h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || h.starts_with('-') {
            out.push(format!("os.hostname: {h:?}: letters, digits and -"));
        }
    }
    out
}

/// A name under /etc/zoneinfo, nothing outside it.
pub fn timezone_ok(zone: &str) -> bool {
    !zone.is_empty() && !zone.starts_with('/') && !zone.split('/').any(|c| c == ".." || c.is_empty()) && zone.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c))
}

/// The computer's people's passwords (`--password-stdin`: `nami apply`'s, `nami usb`'s),
/// by login, for whoever has no hash in the file.
pub(crate) static PASSWORDS: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();

/// The new computer's people's passwords, from stdin: a `login:password`
/// line each, as chpasswd reads them.
pub fn password_from_stdin() -> Result<()> {
    let mut all = std::collections::BTreeMap::new();
    for line in std::io::stdin().lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let (login, password) = line.split_once(':').ok_or_else(|| anyhow!("--password-stdin: a line is login:password, as chpasswd reads"))?;
        ensure!(!password.is_empty(), "--password-stdin: {login}'s password is empty");
        all.insert(login.to_string(), password.to_string());
    }
    ensure!(!all.is_empty(), "--password-stdin: no login:password lines");
    let _ = PASSWORDS.set(all);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_time_zone_is_a_name_under_zoneinfo() {
        assert!(timezone_ok("Europe/Copenhagen") && timezone_ok("UTC") && timezone_ok("America/Argentina/Buenos_Aires") && timezone_ok("Etc/GMT+5"));
        for bad in ["", "/etc/shadow", "../etc/shadow", "Europe/../../etc", "a b", "Europe//Paris", "x;rm"] {
            assert!(!timezone_ok(bad), "{bad}");
        }
    }

    /// A file's text with its `packages: [..]` example packages by path.
    pub(crate) fn listing(text: &str) -> String {
        let Some((head, rest)) = text.split_once("packages: [") else { return text.to_string() };
        let (list, tail) = rest.split_once(']').unwrap();
        let ex = |p: &str| if std::path::Path::new(&crate::config::tests::ex(p)).is_dir() { crate::config::tests::ex(p) } else { p.to_string() };
        let list: Vec<String> = list.split(',').map(|p| ex(p.trim())).collect();
        format!("{head}packages: [{}]{tail}", list.join(", "))
    }

    #[test]
    fn desktops_are_the_packages_with_a_login_screen() {
        crate::config::load_text(&listing("packages: [niri, htop]\n"), "t", Path::new(".")).unwrap();
        assert_eq!(desktops(), ["niri"]);
        let os = Os { desktop: vec!["xfce".into(), "mate".into()], ..Default::default() };
        let errs = check(&os);
        assert!(errs[0].contains("no desktop xfce") && errs[1].contains("listed here: niri"), "{errs:?}");
    }

    #[test]
    fn a_package_listing_a_desktop_only_picks_the_login_screen() {
        if !crate::config::tests::namios_examples() {
            return;
        }
        // namios: niri (with GDM) and namiOS's own login screen,
        // nearer: niri is the desktop, greetd the login screen.
        let c = crate::config::load_text(&listing("packages: [namios]\n"), "t", Path::new(".")).unwrap();
        let os = c.os.unwrap();
        assert_eq!(desktops(), ["niri"]);
        assert_eq!(os.desktop, ["niri"]);
        assert_eq!(os.login_screen.as_deref(), Some("greetd"));
        // niri's GDM, unused, is left out, with GNOME's shell it brings.
        assert!(!os.packages.iter().any(|p| p == "gdm" || p == "gnome-shell"), "{:?}", os.packages);
        // (Its folder builds nami's niri: listed by its path.)
        assert!(os.packages.iter().any(|p| p.ends_with("/niri")));
    }

    /// A desktop of someone's own next to namiOS's: `desk`, with GDM, in
    /// `dir`.
    pub(crate) fn a_desk(dir: &Path) {
        std::fs::create_dir_all(dir.join("desk")).unwrap();
        std::fs::write(dir.join("desk/package.nami"), listing("packages: [base, gdm, foot]\n")).unwrap();
    }

    #[test]
    fn namios_with_another_desktop() {
        if !crate::config::tests::namios_examples() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nami-another-desktop-{}", std::process::id()));
        a_desk(&dir);
        let c = crate::config::load_text(&listing("packages: [namios-bare, ./desk, git]\ndesktop: desk\nlogin-screen: greetd\nusers: { ada: { admin: true } }\n"), "t", &dir).unwrap();
        let os = c.os.unwrap();
        // Yours first: the default session.
        assert_eq!(os.desktop, ["desk", "niri"]);
        assert!(check(&os).is_empty(), "{:?}", check(&os));
        // The file's packages are its own; what the computer is made of is
        // all its sets bring (namios-bare's, and desk's).
        // (niri's folder builds nami's niri: listed by its path.)
        assert_eq!(c.packages.iter().map(|p| p.rsplit('/').next().unwrap()).collect::<Vec<_>>(), ["namios-bare", "desk", "git"]);
        for p in ["niri", "nautilus", "firefox", "foot", "systemd"] {
            assert!(os.packages.iter().any(|x| x.rsplit('/').next() == Some(p)), "no {p}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn checks() {
        crate::config::load_text(&listing("packages: [niri]\n"), "t", Path::new(".")).unwrap();
        let os = |y: &str| serde_yaml::from_str::<Os>(y).unwrap();
        assert!(check(&os("desktop: xfce\nusers: { a: {} }"))[0].contains("no desktop xfce"));
        assert!(check(&os("desktop: niri")).is_empty());
        assert!(check(&os("{}")).is_empty(), "no desktop: a text console");
        assert!(!check(&os("desktop: niri\nusers: { Ada: {} }")).is_empty());
        assert!(!check(&os("desktop: niri\nusers: { ada: { password: hunter2 } }")).is_empty());
        assert!(serde_yaml::from_str::<Os>("desktops: [niri]").is_err());
        // A child: not an admin too, and limits only for one.
        assert!(check(&os("users: { ada: { child: true, hours: 07:00-20:00, without: [discord] } }")).is_empty());
        assert!(check(&os("users: { ada: { child: true, admin: true } }"))[0].contains("can't be an admin too"));
        assert!(check(&os("users: { ada: { hours: 07:00-20:00 } }"))[0].contains("ada.hours: limits are for a child"));
        assert!(check(&os("users: { ada: { without: [steam] } }"))[0].contains("ada.without"));
        assert!(check(&os("users: { ada: { child: true, hours: 7-20 } }"))[0].contains("like 07:00-20:00"));
        assert!(check(&os("users: { ada: { groups: [children] } }"))[0].contains("child: true"));
    }

    #[test]
    fn a_childs_hours() {
        assert_eq!(parse_hours("07:00-20:00"), Some((25200, 72000)));
        assert_eq!(parse_hours("06:30-24:00"), Some((23400, 86400)));
        assert_eq!(parse_hours(" 07:00 - 20:30 "), Some((25200, 73800)));
        for bad in ["20:00-07:00", "07:00-07:00", "7:00-20:00", "07:60-20:00", "24:30-25:00", "07:00", "now-later"] {
            assert_eq!(parse_hours(bad), None, "{bad}");
        }
    }
}
