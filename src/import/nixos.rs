//! NixOS settings that have a place on any Linux distribution: kernel and device
//! settings as /etc drop-ins (sysctl.d, modules-load.d, modprobe.d,
//! tmpfiles.d, udev rules), system-wide environment variables, and what
//! NixOS's `programs.*` modules install and start besides their package.

use serde_yaml::{Mapping, Value};

use super::Reader;
use crate::nix::expr::Val;

pub const SYSCTL: &str = "/etc/sysctl.d/60-nami.conf";
pub const MODULES_LOAD: &str = "/etc/modules-load.d/60-nami.conf";
pub const MODPROBE: &str = "/etc/modprobe.d/60-nami.conf";
pub const TMPFILES: &str = "/etc/tmpfiles.d/60-nami.conf";
pub const UDEV_RULES: &str = "/etc/udev/rules.d/99-nami.rules";
pub const ENVIRONMENT_D: &str = "/etc/environment.d/61-nami-env.conf"; // 60-nami.conf is the installer's
pub const PROFILE_D: &str = "/etc/profile.d/nami-env.sh";

/// Wayland compositors and desktops whose NixOS module also registers a login
/// session: nami installs the program, but login screens list sessions from
/// /usr/share/{wayland-,x}sessions.
const SESSIONS: &[&str] = &["niri", "hyprland", "sway", "river", "wayfire", "labwc", "miracle-wm", "cosmic", "uwsm", "i3", "qtile"];

/// NixOS `programs.<name>` modules: packages they add besides their own, and
/// a user unit their package ships that the module starts.
struct Program {
    name: &'static str,
    packages: &'static [&'static str],
    /// (unit, package) enabled for every user unless `systemd.enable = false`.
    user_unit: Option<(&'static str, &'static str)>,
    /// Options that only add a package when on.
    flags: &'static [(&'static str, &'static str)],
    /// Options with nothing to convert (built in, or the module's internals).
    quiet: &'static [&'static str],
}

const PROGRAMS: &[Program] = &[
    Program {
        name: "dms-shell",
        packages: &["quickshell", "matugen", "cava"],
        user_unit: Some(("dms", "dux-shell")),
        flags: &[("enableSystemMonitoring", "dux-gop"), ("enableClipboardPaste", "wtype"), ("enableCalendarEvents", "khal")],
        quiet: &["systemd", "enableVPN", "enableDynamicTheming", "enableAudioWavelength", "enableBrightnessControl", "enableColorPicker", "enableSystemSound", "enableClipboard", "quickshell"],
    },
    Program { name: "hyprland", packages: &["xdg-desktop-portal-hyprland"], user_unit: None, flags: &[], quiet: &["xwayland", "withUWSM", "portalPackage"] },
    Program { name: "sway", packages: &["swaylock", "swayidle", "foot", "wmenu", "grim", "brightnessctl"], user_unit: None, flags: &[], quiet: &["wrapperFeatures", "xwayland"] },
    Program { name: "niri", packages: &["xdg-desktop-portal-gnome"], user_unit: None, flags: &[], quiet: &[] },
    Program { name: "thunar", packages: &[], user_unit: None, flags: &[], quiet: &[] },
    Program { name: "steam", packages: &[], user_unit: None, flags: &[], quiet: &["remotePlay", "dedicatedServer", "localNetworkGameTransfers", "gamescopeSession", "extest", "protontricks"] },
];

impl Reader {
    /// NixOS `programs.<name>`: what its module does besides installing the
    /// package. True: the option is handled.
    pub(super) fn nixos_program_option(&mut self, name: &str, k: &str, v: &Val) -> bool {
        let what = format!("programs.{name}.{k}");
        let on = matches!(v, Val::Bool(true));
        if let Some(p) = PROGRAMS.iter().find(|p| p.name == name) {
            if let Some((_, pkg)) = p.flags.iter().find(|(f, _)| *f == k) {
                if on {
                    self.found.package(pkg, &what);
                }
                return true;
            }
            if p.quiet.contains(&k) {
                return true;
            }
        }
        match (name, k) {
            (_, "openFirewall") => {
                if on {
                    self.found.skip(what, "nami doesn't manage the firewall: open its ports with the distro's firewall tool (ufw, firewalld)");
                }
            }
            ("firefox", "policies") => match self.ev.to_yaml(v) {
                Ok(p) => {
                    let policies = Value::Mapping(Mapping::from_iter([(Value::String("policies".into()), p)]));
                    self.found.etc_files.entry("/etc/firefox/policies/policies.json".into()).or_insert(policies);
                }
                Err(why) => self.found.skip(what, why),
            },
            ("chromium", "extraOpts") => match self.ev.to_yaml(v) {
                Ok(p) => {
                    self.found.etc_files.entry("/etc/chromium/policies/managed/nami.json".into()).or_insert(p);
                }
                Err(why) => self.found.skip(what, why),
            },
            ("git", "config") => match self.ev.to_yaml(v) {
                // A list of sets is NixOS's way to order sections; one set is the usual.
                Ok(c @ Value::Mapping(_)) => {
                    self.found.etc_files.entry("/etc/gitconfig".into()).or_insert(c);
                }
                Ok(Value::Sequence(list)) => {
                    let mut all = Mapping::new();
                    for item in list {
                        if let Value::Mapping(m) = item {
                            super::merge_yaml(&mut all, m);
                        }
                    }
                    self.found.etc_files.entry("/etc/gitconfig".into()).or_insert(Value::Mapping(all));
                }
                Ok(_) => self.found.skip(what, "not a set of settings"),
                Err(why) => self.found.skip(what, why),
            },
            ("tmux", "extraConfig") => match v {
                Val::Str(text, true) => {
                    self.found.etc_files.entry("/etc/tmux.conf".into()).or_insert(Value::String(text.clone()));
                }
                _ => self.found.skip(what, "refers to values only Nix knows"),
            },
            ("zsh" | "bash" | "fish", _) => self.found.skip(what, format!("{name} set up for every user: put it in each person's user.nami, under shell.{name}")),
            _ => return false,
        }
        true
    }

    /// After a NixOS `programs.<name>` is read: what its module adds.
    pub(super) fn nixos_program_extras(&mut self, name: &str, m: &std::collections::BTreeMap<String, crate::nix::expr::Thunk>) {
        let what = format!("programs.{name}");
        if SESSIONS.contains(&name) {
            self.found.warn(format!(
                "{what}: the login screen lists sessions from /usr/share/wayland-sessions, so it only offers {name} if the distro's own {name} package is installed (nami's works once you're in it)"
            ));
        }
        let Some(p) = PROGRAMS.iter().find(|p| p.name == name) else { return };
        for pkg in p.packages {
            self.found.package(pkg, &format!("{what}.enable"));
        }
        if let Some((unit, pkg)) = p.user_unit {
            let systemd = m.get("systemd").map(|t| self.force(t));
            let off = systemd.as_ref().and_then(|s| self.ev.select(s, "enable")).is_some_and(|e| matches!(e, Val::Bool(false)));
            if !off {
                let value = Value::Mapping(Mapping::from_iter([(Value::String("package".into()), Value::String(pkg.into()))]));
                self.add_service(unit, value, &what, true);
            }
        }
    }

    /// NixOS settings for the OS itself (timezone, hostname, locale, keymap):
    /// the distro keeps them, so what's listed is the command that sets each
    /// once. False: nothing specific to say.
    pub(super) fn nixos_os(&mut self, k: &str, v: &Val) -> bool {
        let get = |r: &Self, path: &[&str]| {
            let mut cur = v.clone();
            for p in path {
                cur = r.ev.select(&cur, p)?;
            }
            Some(cur)
        };
        let text = |r: &Self, path: &[&str]| get(r, path).and_then(|v| v.str().map(String::from));
        let mut said = false;
        let mut hint = |r: &mut Self, what: &str, why: String| {
            r.found.skip(what, why);
            said = true;
        };
        match k {
            "time" => {
                if let Some(tz) = text(self, &["timeZone"]) {
                    hint(self, "time.timeZone", format!("the distro's: set it once with `sudo timedatectl set-timezone {tz}`"));
                }
                if matches!(get(self, &["hardwareClockInLocalTime"]), Some(Val::Bool(true))) {
                    hint(self, "time.hardwareClockInLocalTime", "the distro's: `sudo timedatectl set-local-rtc 1`".into());
                }
            }
            "networking" => {
                if let Some(h) = text(self, &["hostName"]) {
                    hint(self, "networking.hostName", format!("the distro's: set it once with `sudo hostnamectl set-hostname {h}`"));
                }
                if let Some(hosts) = text(self, &["extraHosts"]) {
                    hint(self, "networking.extraHosts", format!("/etc/hosts is the distro's: add these lines to it: {}", hosts.trim().replace('\n', "; ")));
                }
                if get(self, &["firewall"]).is_some() {
                    hint(self, "networking.firewall", "nami doesn't manage the firewall: use the distro's (ufw, firewalld)".into());
                }
                let rest: Vec<String> = self.attrs(v).into_iter().map(|(k, _)| k).filter(|k| !["hostName", "extraHosts", "firewall", "hostId"].contains(&k.as_str())).collect();
                if !rest.is_empty() {
                    hint(self, "networking", format!("{}: networking is the distro's (NetworkManager on most desktops)", rest.join(", ")));
                }
            }
            "i18n" => {
                let mut vars: Vec<String> = text(self, &["defaultLocale"]).map(|l| vec![format!("LANG={l}")]).unwrap_or_default();
                for (lc, t) in get(self, &["extraLocaleSettings"]).map(|e| self.attrs(&e)).unwrap_or_default() {
                    if let Some(l) = self.force(&t).str() {
                        vars.push(format!("{lc}={l}"));
                    }
                }
                if !vars.is_empty() {
                    hint(self, "i18n", format!("the distro's: set it once with `sudo localectl set-locale {}` (the locales have to be generated: locale-gen on Debian and Arch)", vars.join(" ")));
                }
                if get(self, &["inputMethod"]).is_some() {
                    hint(self, "i18n.inputMethod", "the distro's input method framework (ibus, fcitx5): install and pick it in its settings".into());
                }
            }
            "console" => {
                if let Some(km) = text(self, &["keyMap"]) {
                    hint(self, "console.keyMap", format!("the distro's: `sudo localectl set-keymap {km}`"));
                }
            }
            _ => {}
        }
        said
    }

    /// `services.xserver.xkb`: the keyboard layout, set with localectl.
    pub(super) fn xkb_hint(&mut self, xserver: &Val) {
        let Some(xkb) = self.ev.select(xserver, "xkb") else { return };
        let Some(layout) = self.ev.select(&xkb, "layout").and_then(|l| l.str().map(String::from)) else { return };
        let variant = self.ev.select(&xkb, "variant").and_then(|l| l.str().map(String::from)).unwrap_or_default();
        let cmd = if variant.is_empty() { format!("sudo localectl set-x11-keymap {layout}") } else { format!("sudo localectl set-x11-keymap {layout} \"\" {variant}") };
        self.found.skip("services.xserver.xkb", format!("the keyboard layout is the distro's: `{cmd}`"));
    }

    /// From a NixOS configuration into your user file: the user units a
    /// `programs.<name>` module starts for everyone, as your own.
    pub(super) fn nixos_user_units(&mut self, name: &str, v: &Val) {
        let Some(p) = PROGRAMS.iter().find(|p| p.name == name) else { return };
        let Val::Attrs(m) = v else { return };
        if !matches!(m.get("enable").map(|t| self.force(t)), Some(Val::Bool(true))) {
            return;
        }
        if p.user_unit.is_some() {
            let packages = std::mem::take(&mut self.found.packages);
            self.nixos_program_extras(name, m);
            // Only the unit: the packages are the computer's.
            self.found.packages = packages;
        }
    }

    /// `boot.*`: the kernel settings that are /etc drop-ins on any distro.
    /// The bootloader, kernel and initrd are the distro's.
    pub(super) fn boot(&mut self, v: &Val) {
        for (k, t) in self.attrs(v) {
            let bv = self.force(&t);
            if matches!(bv, Val::Null) {
                continue;
            }
            let what = format!("boot.{k}");
            match k.as_str() {
                "kernel" => {
                    for (kk, t) in self.attrs(&bv) {
                        let kv = self.force(&t);
                        match kk.as_str() {
                            "sysctl" => {
                                let mut lines = String::new();
                                for (key, t) in self.attrs(&kv) {
                                    match self.force(&t) {
                                        Val::Str(s, true) => lines += &format!("{key} = {s}\n"),
                                        Val::Int(n) => lines += &format!("{key} = {n}\n"),
                                        Val::Bool(b) => lines += &format!("{key} = {}\n", u8::from(b)),
                                        Val::Null => {}
                                        _ => self.found.skip(format!("boot.kernel.sysctl.\"{key}\""), "refers to values only Nix knows"),
                                    }
                                }
                                if !lines.is_empty() {
                                    self.found.etc_files.entry(SYSCTL.into()).or_insert(Value::String(lines));
                                }
                            }
                            _ => self.found.skip(format!("boot.kernel.{kk}"), "the kernel is the distro's"),
                        }
                    }
                }
                "kernelModules" => match self.strs(&bv) {
                    Some(m) if !m.is_empty() => {
                        self.found.etc_files.entry(MODULES_LOAD.into()).or_insert(Value::String(m.join("\n") + "\n"));
                    }
                    Some(_) => {}
                    None => self.found.skip(what, "refers to values only Nix knows"),
                },
                "blacklistedKernelModules" | "extraModprobeConfig" => {
                    let text = match (k.as_str(), self.strs(&bv)) {
                        ("blacklistedKernelModules", Some(m)) => m.iter().map(|m| format!("blacklist {m}\n")).collect::<String>(),
                        ("extraModprobeConfig", Some(t)) => t.join("\n").trim_end().to_string() + "\n",
                        _ => {
                            self.found.skip(what, "refers to values only Nix knows");
                            continue;
                        }
                    };
                    let entry = self.found.etc_files.entry(MODPROBE.into()).or_insert(Value::String(String::new()));
                    if let Value::String(s) = entry {
                        if !s.is_empty() && !s.ends_with('\n') {
                            s.push('\n');
                        }
                        s.push_str(&text);
                    }
                }
                _ => self.found.skip(what, "the bootloader, kernel and initrd are the distro's"),
            }
        }
    }

    /// `systemd.tmpfiles`: `rules` as they are, `settings` written as rules.
    pub(super) fn tmpfiles(&mut self, v: &Val) {
        let mut lines: Vec<String> = Vec::new();
        for (k, t) in self.attrs(v) {
            let tv = self.force(&t);
            match k.as_str() {
                "rules" => match self.strs(&tv) {
                    Some(r) => lines.extend(r),
                    None => self.found.skip("systemd.tmpfiles.rules", "refers to values only Nix knows"),
                },
                "settings" => {
                    for (group, t) in self.attrs(&tv) {
                        for (path, t) in self.attrs(&self.force(&t)) {
                            for (ty, t) in self.attrs(&self.force(&t)) {
                                let e = self.force(&t);
                                let field = |r: &Self, f: &str| match r.ev.select(&e, f) {
                                    Some(Val::Str(s, true)) if !s.is_empty() => Some(s),
                                    Some(Val::Int(n)) => Some(n.to_string()),
                                    Some(Val::Null) | None => Some("-".into()),
                                    Some(Val::Str(..)) => Some("-".into()),
                                    _ => None,
                                };
                                let fields: Option<Vec<String>> = ["mode", "user", "group", "age", "argument"].iter().map(|f| field(self, f)).collect();
                                match fields {
                                    Some(f) => lines.push(format!("{ty} {path} {}", f.join(" ")).trim_end_matches(" -").to_string()),
                                    None => self.found.skip(format!("systemd.tmpfiles.settings.{group}.\"{path}\""), "refers to values only Nix knows"),
                                }
                            }
                        }
                    }
                }
                "packages" => self.found.skip("systemd.tmpfiles.packages", "tmpfiles.d rules from packages: copy the ones you need into a files: entry under /etc/tmpfiles.d"),
                _ => self.found.skip(format!("systemd.tmpfiles.{k}"), "no nami equivalent"),
            }
        }
        if !lines.is_empty() {
            self.found.etc_files.entry(TMPFILES.into()).or_insert(Value::String(lines.join("\n") + "\n"));
        }
    }

    /// `services.udev`: rules as a rules file; the rest is the distro's.
    pub(super) fn udev(&mut self, m: &std::collections::BTreeMap<String, crate::nix::expr::Thunk>) {
        for (k, t) in m {
            let v = self.force(t);
            match (k.as_str(), &v) {
                (_, Val::Null) => {}
                ("extraRules", Val::Str(rules, true)) => {
                    self.found.etc_files.entry(UDEV_RULES.into()).or_insert(Value::String(rules.clone()));
                }
                ("extraRules", _) => self.found.skip("services.udev.extraRules", "refers to values only Nix knows"),
                ("packages", _) => {
                    for item in self.ev.list(&v) {
                        if let Val::Pkg(p) = &item {
                            self.found.warn(format!("services.udev.packages: {p} is installed, but udev only reads rules from /etc and /usr: copy its lib/udev/rules.d files into a files: entry under /etc/udev/rules.d"));
                        }
                        self.package(item, "services.udev.packages", false);
                    }
                }
                ("enable", _) => {}
                _ => self.found.skip(format!("services.udev.{k}"), "part of the OS, which the distro sets up"),
            }
        }
    }
}

/// Environment variables for the whole computer, as NixOS sets them: for
/// desktop sessions (environment.d) and login shells (profile.d).
pub fn env_files(env: &std::collections::BTreeMap<String, String>) -> [(&'static str, String); 2] {
    let mut envd = String::from("# from your nami file (imported from NixOS's environment.variables / sessionVariables)\n");
    let mut sh = envd.clone();
    for (k, v) in env {
        envd += &format!("{k}={v}\n");
        sh += &format!("export {k}={}\n", crate::home::env_sh(v));
    }
    [(ENVIRONMENT_D, envd), (PROFILE_D, sh)]
}
