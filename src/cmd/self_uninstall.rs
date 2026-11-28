//! `sudo nami self-uninstall`: nami itself off this computer, as its uninstaller takes it
//! (uninstall.sh in nami's folder; a Mac's is uninstall-mac.sh, put there as uninstall.sh).
//! First it says plainly that it's nami itself, not a package, what goes and what stays, and
//! waits for the word `uninstall` (--yes: scripts, CI, nami's app once it has asked). Removing
//! one app is `nami remove` (`nami uninstall`, as other package managers say it).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{migrate, nami_os, profile, style};

/// The word to type.
pub(crate) const WORD: &str = "uninstall";

/// Where it's said how to, for terminal users.
pub(crate) const COMMAND: &str = "sudo nami self-uninstall";

/// Said when it stops before anything's removed.
pub(crate) const NOTHING: &str = "Nothing was removed.";

/// What this computer's nami has: where it keeps its store, and whether Nix shares it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Here {
    pub mac: bool,
    /// Nix is installed too (its database): nami's store paths are left to it.
    pub nix: bool,
    /// A Mac's /nix volume is nami's own (its LaunchDaemon mounts it).
    pub own_volume: bool,
}

impl Here {
    fn of(root: &Path) -> Here {
        Here {
            mac: crate::index::MAC,
            nix: root.join("nix/var/nix/db/db.sqlite").exists(),
            own_volume: root.join("Library/LaunchDaemons/dev.namios.store.plist").exists(),
        }
    }
}

/// `nami self-uninstall firefox`: not this. What it says instead.
pub(crate) fn stray(args: &[String]) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let names = args.join(" ");
    Some(format!("nami self-uninstall removes nami itself. To remove {names}, run: nami remove {names}"))
}

/// Whether what was typed goes ahead: the word, nothing else (case and spaces around aside).
pub(crate) fn confirmed(answer: &str) -> bool {
    answer.trim().eq_ignore_ascii_case(WORD)
}

/// What's said before it asks: that it's nami itself, what goes, what stays, as the
/// uninstallers do it.
pub(crate) fn warning(p: style::Paint, here: Here) -> String {
    let computer = if here.mac { "Mac" } else { "computer" };
    let store = match (here.mac, here.nix, here.own_volume) {
        (true, false, true) => "its store (the Nix Store volume, /nix)",
        (_, true, _) => "its part of the store (Nix keeps /nix and its own)",
        _ => "its store (/nix/store)",
    };
    let mut s = String::new();
    s += &format!("{} {}\n\n", p.warning("warning:"), p.bold(&format!("This uninstalls nami itself from this {computer}, not a package: nami, every app and tool installed with it, and {store}.")));
    s += "It removes:\n";
    let apps = if here.nix {
        format!("  - every app and tool installed with nami, for everyone on this {computer} (Nix's nix-collect-garbage frees them)\n")
    } else {
        format!("  - every app and tool installed with nami, for everyone on this {computer}\n")
    };
    if here.mac {
        s += "  - nami's command, the nami app in Applications, and nami's background service\n";
        s += &apps;
        s += "  - the apps nami put in Applications, in the nami Apps folders (what's yours there stays)\n";
        s += "  - nami's lines in the shells' startup files, and its Tab completion\n";
    } else {
        s += "  - nami's command, the nami app, and nami's background service and daily update\n";
        s += &apps;
        s += "  - the services and /etc files nami set up (files they replaced come back)\n";
        s += "  - nami's lines in the shells' startup files, its app menu entries and Tab completion\n";
    }
    s += "It keeps:\n";
    if here.mac {
        s += "  - your nami files: ~/user.nami and ~/.config/nami\n";
    } else {
        s += "  - your nami files: ~/user.nami, ~/.config/nami, and the system's in /etc/nami\n";
    }
    s += "  - your home folder and everything in it\n";
    if !(here.mac && here.own_volume && !here.nix) {
        s += "  - anything in /nix/store that isn't nami's\n";
    }
    s += &format!("\nTo remove one app instead, use {}.\n", p.accent("nami remove <name>"));
    s
}

/// The prompt, after the warning.
pub(crate) fn prompt(p: style::Paint) -> String {
    format!("\nType {} to uninstall nami, or press Return to cancel: ", p.bold(WORD))
}

/// nami's uninstaller on this computer, in its folder (/nami, or /nix/var/nami; a Mac's
/// /nix/var/nami).
pub(crate) fn script(root: &Path) -> PathBuf {
    migrate::var_dir(root).join("uninstall.sh")
}

/// nami from a .deb or .rpm (/opt/nami): removed as it came, by the package manager, whose
/// removal runs the release's uninstall.sh. The program and its arguments.
fn by_package(root: &Path) -> Option<(&'static str, Vec<&'static str>)> {
    if root != Path::new("/") || !root.join("opt/nami/uninstall.sh").is_file() {
        return None;
    }
    let ok = |prog: &str, args: &[&str]| std::process::Command::new(prog).args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    if ok("dpkg-query", &["-W", "-f=${Status}", "nami"]).is_some_and(|s| s.contains("install ok installed")) {
        return Some(("dpkg", vec!["--remove", "nami"]));
    }
    if ok("rpm", &["-q", "nami"]).is_some() {
        return Some(("rpm", vec!["-e", "nami"]));
    }
    None
}

/// What someone who isn't root is told. A Mac's account that isn't an administrator can't
/// use sudo: nami's app asks for an administrator's name and password instead.
fn needs_admin(mac: bool) -> String {
    let mut s = format!("Uninstalling nami needs an administrator: run {COMMAND}");
    if mac {
        s.push_str(", or in nami's app choose nami › Uninstall nami…");
    }
    s
}

/// Say it, and stop with `code`.
fn stop(text: &str, code: i32) -> ! {
    eprintln!("{}", style::err().line(text));
    std::process::exit(code)
}

pub(crate) fn run(root: &Path, yes: bool, stray_args: Vec<String>) -> Result<()> {
    // `nami self-uninstall firefox`: someone after one app.
    if let Some(said) = stray(&stray_args) {
        stop(&format!("error: {said}"), 2);
    }
    if nami_os::here() {
        stop("error: On namiOS, nami is the system itself, so it can't be uninstalled.", 1);
    }
    if !profile::is_root() {
        stop(&needs_admin(cfg!(target_os = "macos")), 77);
    }
    let script = script(root);
    let package = by_package(root);
    if package.is_none() && !script.is_file() {
        stop(&format!("error: nami's uninstaller isn't here ({}). Install nami again (curl -fsSL https://namios.dev/install | sh), then uninstall it.", script.display()), 1);
    }
    let p = style::out();
    if !yes {
        print!("{}{}", warning(p, Here::of(root)), prompt(p));
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        // (What was typed ends its line on a terminal; given another way, it's a line of its own.)
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            println!();
        }
        if !confirmed(&answer) {
            stop(NOTHING, 1);
        }
    }
    println!("Uninstalling nami itself from this {}.", if crate::index::MAC { "Mac" } else { "computer" });
    // nami's process gives way to the uninstaller: nothing of nami's runs while its files
    // (and on a Mac, the volume they're on) go.
    use std::os::unix::process::CommandExt;
    let err = match package {
        Some((prog, args)) => {
            println!("nami came as a package: removing it with {prog}.");
            std::process::Command::new(prog).args(args).current_dir("/").exec()
        }
        None => std::process::Command::new("/bin/sh").arg(&script).current_dir("/").exec(),
    };
    Err(anyhow::anyhow!("couldn't run nami's uninstaller ({}): {err}", script.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(here: Here) -> String {
        warning(style::Paint(false), here)
    }

    #[test]
    fn a_package_is_not_nami() {
        assert_eq!(stray(&[]), None);
        assert_eq!(stray(&["firefox".into()]).as_deref(), Some("nami self-uninstall removes nami itself. To remove firefox, run: nami remove firefox"));
        assert_eq!(stray(&["gimp".into(), "vlc".into()]).as_deref(), Some("nami self-uninstall removes nami itself. To remove gimp vlc, run: nami remove gimp vlc"));
    }

    #[test]
    fn a_mac_is_pointed_at_the_app_too() {
        assert_eq!(needs_admin(false), "Uninstalling nami needs an administrator: run sudo nami self-uninstall");
        assert!(needs_admin(true).ends_with(", or in nami's app choose nami › Uninstall nami…"));
    }

    #[test]
    fn only_the_word_goes_ahead() {
        assert!(confirmed("uninstall\n"));
        assert!(confirmed("  Uninstall \r\n"));
        for no in ["", "\n", "y\n", "yes\n", "uninstal\n", "uninstall nami\n", "remove\n"] {
            assert!(!confirmed(no), "{no:?}");
        }
    }

    #[test]
    fn it_says_its_nami_itself_and_what_stays() {
        let linux = plain(Here { mac: false, nix: false, own_volume: false });
        assert!(linux.starts_with("warning: This uninstalls nami itself from this computer, not a package: nami, every app and tool installed with it, and its store (/nix/store)."), "{linux}");
        assert!(linux.contains("~/user.nami, ~/.config/nami, and the system's in /etc/nami"));
        assert!(linux.contains("your home folder and everything in it"));
        assert!(linux.contains("To remove one app instead, use nami remove <name>."));
        assert!(linux.contains("the services and /etc files nami set up"));
        let mac = plain(Here { mac: true, nix: false, own_volume: true });
        assert!(mac.contains("from this Mac, not a package") && mac.contains("the Nix Store volume, /nix"), "{mac}");
        assert!(mac.contains("the nami app in Applications"));
        assert!(!mac.contains("/etc/nami") && !mac.contains("anything in /nix/store"));
        // Nix shares the store: only nami's part of it, and Nix's own stays.
        let shared = plain(Here { mac: true, nix: true, own_volume: true });
        assert!(shared.contains("its part of the store (Nix keeps /nix and its own)") && shared.contains("nix-collect-garbage"), "{shared}");
        for t in [&linux, &mac, &shared] {
            assert!(!t.to_lowercase().contains("tray"));
        }
        assert!(prompt(style::Paint(false)).contains("Type uninstall to uninstall nami"));
    }

    #[test]
    fn the_uninstaller_is_in_namis_folder() {
        let root = std::env::temp_dir().join(format!("nami-self-uninstall-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nix/var/nami")).unwrap();
        assert_eq!(script(&root), root.join("nix/var/nami/uninstall.sh"));
        assert_eq!(by_package(&root), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
