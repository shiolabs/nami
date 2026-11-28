//! Updates by themselves on another distribution (Ubuntu, Fedora...), as
//! namiOS has them: nami-update.timer runs `nami os-update` once a day, as
//! root, which gets the newest nami (src/selfupdate.rs), then the newest
//! catalog and the system's apps at their newest; whoever's at a desktop
//! is told what changed, or that updating has stopped working.
//!
//! system.nami says whether: `updates: manual` takes the timer
//! away (src/config/). install.sh sets it up (`nami os-update --set-up`),
//! so a computer with an older nami gets it as it updates to this one (the
//! release's setup.sh runs install.sh, from `nami self-update`, a .deb or
//! an .rpm; on NixOS setup.sh itself); `nami apply` of system.nami
//! after that. On NixOS the timer is in /etc/systemd/system.control, as
//! nami's daemon is: /etc/systemd/system is NixOS's own, read-only. On
//! namiOS the base package has the timer, and update-notify tells people.
//!
//! On a Mac it's a LaunchDaemon instead, dev.namios.update
//! (/Library/LaunchDaemons/dev.namios.update.plist): launchd runs the same
//! `nami os-update` once a day, and after the Mac wakes when it was asleep
//! then. install-mac.sh sets it up, uninstall-mac.sh takes it away; whoever's
//! at the screen is told, with a notification.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::config::UPDATE_TIMER;
use crate::store::Store;

const SERVICE: &str = "nami-update.service";

/// The service, for the computer at `root` (nami where it is there: /nami/bin/nami).
fn service_text(root: &Path) -> String {
    format!(
        "\
# Made by nami (nami os-update --set-up): system.nami's `updates:` says whether.
[Unit]
Description=Updates nami and the system's apps
Wants=network-online.target
After=network-online.target
[Service]
Type=oneshot
ExecStart={}/bin/nami os-update
Nice=19
IOSchedulingClass=idle
TimeoutStartSec=3h
",
        crate::migrate::seen(root).display()
    )
}

const TIMER_TEXT: &str = "\
# Made by nami (nami os-update --set-up): system.nami's `updates:` says whether.
[Unit]
Description=Updates nami and the system's apps, daily
[Timer]
OnCalendar=daily
RandomizedDelaySec=2h
Persistent=true
[Install]
WantedBy=timers.target
";

/// How long updating may fail before people are told: a computer that's
/// offline for a day needn't say so.
const TELL_AFTER: u64 = 7 * 86400;

/// Where the timer goes, under `root`: the folder on disk, and as systemd
/// sees it (what the enabling link points at). On NixOS that's
/// /etc/systemd/system.control: its /etc/systemd/system is NixOS's own,
/// and read-only (src/services.rs).
fn units(root: &Path) -> (PathBuf, &'static str) {
    let logical = crate::services::local_units(root);
    (root.join(logical.trim_start_matches('/')), logical)
}

/// Every place the timer may be, under `root`: where it goes, and
/// /etc/systemd/system, where it went before.
fn all_units(root: &Path) -> Vec<PathBuf> {
    let mut all = vec![units(root).0];
    let plain = root.join("etc/systemd/system");
    if !all.contains(&plain) {
        all.push(plain);
    }
    all
}

/// The timer (and its service) in place and started, or taken away: `on`
/// is system.nami's `updates:`. Only with systemd (on namiOS the
/// base package has the timer: callers ask there). Whether anything changed.
pub fn set_up(root: &Path, on: bool) -> Result<bool> {
    if crate::index::MAC {
        return set_up_launchd(root, on);
    }
    set_up_systemd(root, on)
}

/// [`set_up`] with systemd: the timer and its service.
fn set_up_systemd(root: &Path, on: bool) -> Result<bool> {
    if !root.join("etc/systemd").is_dir() {
        return Ok(false);
    }
    let (dir, logical) = units(root);
    let wants = dir.join("timers.target.wants").join(UPDATE_TIMER);
    let target = Path::new(logical).join(UPDATE_TIMER);
    let mut changed = false;
    if on {
        std::fs::create_dir_all(wants.parent().unwrap()).with_context(|| format!("making {}", dir.display()))?;
        for (name, text) in [(SERVICE, service_text(root)), (UPDATE_TIMER, TIMER_TEXT.to_string())] {
            let f = dir.join(name);
            if std::fs::read_to_string(&f).ok() != Some(text.clone()) {
                std::fs::write(&f, text).with_context(|| format!("writing {}", f.display()))?;
                changed = true;
            }
        }
        if std::fs::read_link(&wants).ok().as_deref() != Some(target.as_path()) {
            let _ = std::fs::remove_file(&wants);
            std::os::unix::fs::symlink(&target, &wants).with_context(|| format!("turning on {UPDATE_TIMER}"))?;
            changed = true;
        }
    } else {
        // (One nami didn't make, someone's own, stays.) Wherever it is.
        for dir in all_units(root) {
            for f in [dir.join("timers.target.wants").join(UPDATE_TIMER), dir.join(UPDATE_TIMER), dir.join(SERVICE)] {
                let ours = f.is_symlink() || std::fs::read_to_string(&f).is_ok_and(|t| t.starts_with("# Made by nami"));
                if ours && std::fs::remove_file(&f).is_ok() {
                    changed = true;
                }
            }
        }
    }
    // The running systemd, told: the timer goes now, not at the next start.
    if changed && root == Path::new("/") && Path::new("/run/systemd/system").is_dir() {
        let _ = Command::new("systemctl").arg("daemon-reload").status();
        let _ = Command::new("systemctl").arg(if on { "start" } else { "stop" }).arg(UPDATE_TIMER).stderr(Stdio::null()).status();
    }
    Ok(changed)
}

/// A Mac's daily update: launchd's job, and its file. (Its comment, as any XML's, has no
/// double hyphen in it: `--set-up` would make the file one launchd can't read.)
const LABEL: &str = "dev.namios.update";
const MADE_BY: &str = "<!-- Made by nami (nami os-update)";

/// Set in the job's environment: `nami os-update` (and what it runs: a new release's
/// install-mac.sh, its set-up) is launchd's job, which booting the job out would end.
const IN_JOB: &str = "NAMI_DAILY_UPDATE";

/// The job's file, under `root`.
fn plist(root: &Path) -> PathBuf {
    root.join("Library/LaunchDaemons").join(format!("{LABEL}.plist"))
}

/// The job, for the Mac at `root` (nami where it is there: /nix/var/nami/bin/nami). At
/// midnight, as Linux's timer is; launchd runs one missed while the Mac slept when it wakes.
fn plist_text(root: &Path) -> String {
    let nami = crate::migrate::seen(root);
    let nami = nami.display();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
{MADE_BY}: updates nami and the system's apps once a day; system.nami's `updates:` says whether. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{nami}/bin/nami</string>
    <string>os-update</string>
  </array>
  <key>StartCalendarInterval</key>
  <dict>
    <key>Hour</key>
    <integer>0</integer>
    <key>Minute</key>
    <integer>0</integer>
  </dict>
  <key>Nice</key>
  <integer>19</integer>
  <key>LowPriorityIO</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/var/log/nami-update.log</string>
  <key>StandardErrorPath</key>
  <string>/var/log/nami-update.log</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{nami}/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    <key>HOME</key>
    <string>/var/root</string>
    <key>{IN_JOB}</key>
    <string>1</string>
  </dict>
</dict>
</plist>
"#
    )
}

/// [`set_up`] on a Mac: the job's file written (root:wheel, 644) or taken away (only one nami
/// made), and launchd told, on the Mac itself. Whether the file changed.
pub(crate) fn set_up_launchd(root: &Path, on: bool) -> Result<bool> {
    let f = plist(root);
    if !f.parent().unwrap().is_dir() {
        return Ok(false);
    }
    let mut changed = false;
    if on {
        let text = plist_text(root);
        if std::fs::read_to_string(&f).ok() != Some(text.clone()) {
            let new = f.with_extension("plist.new");
            std::fs::write(&new, &text).with_context(|| format!("writing {}", new.display()))?;
            std::fs::set_permissions(&new, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
            // (launchd won't load a daemon's file anyone but root could change.)
            if crate::profile::is_root() {
                std::os::unix::fs::chown(&new, Some(0), Some(0)).with_context(|| format!("making {} root's", new.display()))?;
            }
            std::fs::rename(&new, &f).with_context(|| format!("writing {}", f.display()))?;
            changed = true;
        }
    } else if std::fs::read_to_string(&f).is_ok_and(|t| t.contains(MADE_BY)) {
        // (One nami didn't make, someone's own, stays.)
        std::fs::remove_file(&f).with_context(|| format!("removing {}", f.display()))?;
        changed = true;
    }
    if root == Path::new("/") && crate::index::MAC {
        tell_launchd(&f, on, changed);
    }
    Ok(changed)
}

/// launchd, told what the file says now. Not from inside the job itself, which booting it
/// out would end midway (a new release's install-mac.sh, run by the daily update): the file
/// is as it should be, launchd reads it at the next start, and [`job_still_wanted`] ends a
/// job whose file has gone.
fn tell_launchd(f: &Path, on: bool, changed: bool) {
    if std::env::var_os(IN_JOB).is_some() {
        return;
    }
    let target = format!("system/{LABEL}");
    let launchctl = |args: &[&str]| Command::new("/bin/launchctl").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    let mut loaded = launchctl(&["print", &target]);
    // (Taken away: also one whose file went before, while it was loaded.)
    if loaded && (changed || !on && !f.exists()) {
        launchctl(&["bootout", &target]);
        loaded = false;
    }
    if on && !loaded {
        // (Just booted out, launchd may not take it again for a moment.)
        let file = f.to_string_lossy();
        for _ in 0..20 {
            if launchctl(&["bootstrap", "system", &file]) || launchctl(&["print", &target]) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

/// Whether the daily update should go on: on a Mac, launchd's job ends itself when its file
/// went while it was loaded (`updates: manual`, set up from inside the job). Elsewhere, yes.
pub fn job_still_wanted() -> bool {
    if !crate::index::MAC || std::env::var_os(IN_JOB).is_none() || plist(Path::new("/")).exists() {
        return true;
    }
    println!("updates by themselves: off (updates: manual)");
    let _ = Command::new("/bin/launchctl").args(["bootout", &format!("system/{LABEL}")]).status();
    false
}

/// `nami os-update --set-up`: as system.nami says (no file: on).
pub fn set_up_as_file_says(root: &Path) -> Result<()> {
    if crate::nami_os::here() {
        println!("updates by themselves: as namiOS has them (base, nami-update.timer)");
        return Ok(());
    }
    let text = std::fs::read_to_string(crate::config::system_file()).unwrap_or_default();
    let on = crate::config::updates_itself(&text);
    let changed = set_up(root, on)?;
    println!("updates by themselves: {}{}", if on { "on, daily" } else { "off (updates: manual)" }, if changed { "" } else { " (as it was)" });
    Ok(())
}

/// Where the last update that went well is noted (its time).
fn stamp(store: &Store) -> PathBuf {
    store.var_dir().join("updated")
}

/// The daily update went well: noted, and whoever's at a desktop told what
/// changed (nami itself, from `was`; the system's apps). Not when someone
/// asked for it in Updates (the daemon ran it for them: NAMI_FOR), who sees it there.
pub fn went_well(store: &Store, was: Option<&str>, apps: &[String]) {
    let _ = std::fs::write(stamp(store), b"");
    if std::env::var_os("NAMI_FOR").is_some() {
        return;
    }
    let nami = was.filter(|w| *w != crate::selfupdate::CURRENT).map(|_| format!("nami {}", crate::selfupdate::CURRENT));
    if let Some(body) = said(nami.as_deref(), apps) {
        tell_everyone("Updates Installed", &body);
    }
}

/// What a notice says was updated: nami, and the apps by name (a few, then how many).
fn said(nami: Option<&str>, apps: &[String]) -> Option<String> {
    let apps = match apps {
        [] => None,
        [a] => Some(a.clone()),
        [a, b] => Some(format!("{a} and {b}")),
        [a, b, c] => Some(format!("{a}, {b} and {c}")),
        _ => Some(format!("{} apps", apps.len())),
    };
    match (nami, apps) {
        (None, None) => None,
        (Some(n), None) => Some(format!("{n} is installed.")),
        (None, Some(a)) => Some(format!("{a} {} updated.", if a.contains(" and ") || a.ends_with(" apps") { "were" } else { "was" })),
        (Some(n), Some(a)) => Some(format!("{n} is installed, and {a} {} updated.", if a.contains(" and ") || a.ends_with(" apps") { "were" } else { "was" })),
    }
}

/// The daily update failed: after a week of that, people are told (once a day at most,
/// as the timer goes).
pub fn went_wrong(store: &Store) {
    if std::env::var_os("NAMI_FOR").is_some() {
        return;
    }
    let since = std::fs::metadata(stamp(store)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|e| e.as_secs());
    // (Never went well yet: from when nami was installed.)
    let since = since.or_else(|| std::fs::metadata(store.var_dir().join("bin/nami")).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|e| e.as_secs()));
    if since.is_some_and(|s| s > TELL_AFTER) {
        tell_everyone("Updates Couldn't Be Installed", "This computer hasn't been updated for over a week. Open nami and click Updates to try again.");
    }
}

/// A notification to everyone logged in at a desktop (each one's session bus), as them.
/// On a Mac, to whoever's at the screen.
fn tell_everyone(title: &str, body: &str) {
    if crate::index::MAC {
        return tell_at_the_screen(title, body);
    }
    let Ok(dirs) = std::fs::read_dir("/run/user") else { return };
    for d in dirs.flatten() {
        let Some(uid) = d.file_name().to_str().and_then(|u| u.parse::<u32>().ok()).filter(|u| *u >= 1000) else { continue };
        let bus = d.path().join("bus");
        let Some((_, gid)) = crate::profile::user_by_uid(uid).filter(|_| bus.exists()) else { continue };
        let address = format!("unix:path={}", bus.display());
        let as_them = |c: &mut Command| {
            c.uid(uid).gid(gid).env_clear().env("DBUS_SESSION_BUS_ADDRESS", &address).env("XDG_RUNTIME_DIR", d.path()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        };
        let mut c = Command::new("notify-send");
        c.args(["--app-name=nami", "--icon=system-software-update", "--hint=string:desktop-entry:nami", title, body]);
        as_them(&mut c);
        if c.status().is_ok_and(|s| s.success()) {
            continue;
        }
        // No notify-send: the same, over D-Bus.
        let mut c = Command::new("busctl");
        c.args(["--user", "call", "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications", "Notify", "susssasa{sv}i", "nami", "0", "system-software-update", title, body, "0", "1", "desktop-entry", "s", "nami", "-1"]);
        as_them(&mut c);
        let _ = c.status();
    }
}

/// Who's logged in at a Mac's screen: the owner of /dev/console (nobody, at the login window).
pub fn at_the_screen() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/dev/console").ok().map(|m| m.uid()).filter(|u| *u >= 500)
}

/// A Mac's notification, to whoever's logged in at the screen, in their session, as them.
fn tell_at_the_screen(title: &str, body: &str) {
    let Some(uid) = at_the_screen() else { return };
    let _ = Command::new("/bin/launchctl")
        .args(["asuser", &uid.to_string(), "/usr/bin/sudo", "-u", &format!("#{uid}"), "/usr/bin/osascript"])
        .args(["-e", "on run argv", "-e", "display notification (item 2 of argv) with title (item 1 of argv)", "-e", "end run", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timer_comes_and_goes_with_the_file() {
        let root = std::env::temp_dir().join(format!("nami-auto-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // No systemd: nothing to set up.
        assert!(!set_up_systemd(&root, true).unwrap());
        std::fs::create_dir_all(root.join("etc/systemd")).unwrap();
        assert!(set_up_systemd(&root, true).unwrap());
        let dir = units(&root).0;
        assert_eq!(dir, root.join("etc/systemd/system"));
        assert!(std::fs::read_to_string(dir.join(UPDATE_TIMER)).unwrap().contains("OnCalendar=daily"));
        assert!(std::fs::read_to_string(dir.join(SERVICE)).unwrap().contains("ExecStart=/nami/bin/nami os-update"));
        assert_eq!(std::fs::read_link(dir.join("timers.target.wants").join(UPDATE_TIMER)).unwrap(), Path::new("/etc/systemd/system/nami-update.timer"));
        // Again: as it was.
        assert!(!set_up_systemd(&root, true).unwrap());
        // `updates: manual`: gone, the wanting too.
        assert!(set_up_systemd(&root, false).unwrap());
        assert!(!dir.join(UPDATE_TIMER).exists() && !dir.join(SERVICE).exists() && !dir.join("timers.target.wants").join(UPDATE_TIMER).is_symlink());
        // Someone's own timer of that name stays.
        std::fs::write(dir.join(UPDATE_TIMER), "[Timer]\nOnCalendar=weekly\n").unwrap();
        assert!(!set_up_systemd(&root, false).unwrap());
        assert!(dir.join(UPDATE_TIMER).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// NixOS: /etc/systemd/system is NixOS's own, read-only; the timer goes
    /// where NixOS reads local units from, and is taken from both places.
    #[test]
    fn on_nixos_the_timer_goes_in_system_control() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("nami-auto-update-nixos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (plain, control) = (root.join("etc/systemd/system"), root.join("etc/systemd/system.control"));
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(root.join("etc/NIXOS"), "").unwrap();
        // NixOS's own folder can't be written (as root it could: then nothing's written there either, below).
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(set_up_systemd(&root, true).unwrap());
        assert_eq!(units(&root).0, control);
        assert!(std::fs::read_to_string(control.join(UPDATE_TIMER)).unwrap().contains("OnCalendar=daily"));
        assert!(std::fs::read_to_string(control.join(SERVICE)).unwrap().contains("nami os-update"));
        assert_eq!(std::fs::read_link(control.join("timers.target.wants").join(UPDATE_TIMER)).unwrap(), Path::new("/etc/systemd/system.control/nami-update.timer"));
        assert!(std::fs::read_dir(&plain).unwrap().next().is_none(), "nothing in NixOS's own folder");
        assert!(!set_up_systemd(&root, true).unwrap());
        // Taken away: from system.control, and from /etc/systemd/system (where an older nami put it).
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(plain.join("timers.target.wants")).unwrap();
        std::fs::write(plain.join(UPDATE_TIMER), TIMER_TEXT).unwrap();
        std::fs::write(plain.join(SERVICE), service_text(&root)).unwrap();
        std::os::unix::fs::symlink("/etc/systemd/system/nami-update.timer", plain.join("timers.target.wants").join(UPDATE_TIMER)).unwrap();
        assert!(set_up_systemd(&root, false).unwrap());
        for dir in [&plain, &control] {
            assert!(!dir.join(UPDATE_TIMER).exists() && !dir.join(SERVICE).exists() && !dir.join("timers.target.wants").join(UPDATE_TIMER).is_symlink(), "{}", dir.display());
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A Mac: launchd's job, written and taken away as the timer is (tried here with a scratch
    /// root; launchd itself is only told on the Mac, at /).
    #[test]
    fn on_a_mac_the_job_comes_and_goes_with_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("nami-auto-update-mac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // No /Library/LaunchDaemons: nothing to set up.
        assert!(!set_up_launchd(&root, true).unwrap());
        let daemons = root.join("Library/LaunchDaemons");
        std::fs::create_dir_all(&daemons).unwrap();
        // (A Mac's nami folder: / is read-only there, so /nix/var/nami.)
        std::fs::create_dir_all(root.join("nix/var/nami/bin")).unwrap();
        assert!(set_up_launchd(&root, true).unwrap());
        let f = daemons.join("dev.namios.update.plist");
        assert_eq!(f, plist(&root));
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.starts_with("<?xml") && text.contains(MADE_BY), "{text}");
        assert!(text.contains("<string>dev.namios.update</string>"));
        assert!(text.contains("<string>/nix/var/nami/bin/nami</string>\n    <string>os-update</string>"), "{text}");
        assert!(text.contains("<key>StartCalendarInterval</key>") && text.contains("<key>Hour</key>\n    <integer>0</integer>"));
        assert!(text.contains("<string>/var/log/nami-update.log</string>"));
        assert!(text.contains("<string>/nix/var/nami/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>"));
        assert!(text.contains(&format!("<key>{IN_JOB}</key>")));
        // (No double hyphen in the comment, which XML doesn't allow; every tag closed.)
        let comment = text.lines().nth(2).unwrap();
        assert!(comment.starts_with("<!--") && comment.ends_with("-->") && !comment[4..comment.len() - 3].contains("--"), "{comment}");
        for tag in ["dict", "array", "plist"] {
            assert_eq!(text.matches(&format!("<{tag}")).count(), text.matches(&format!("</{tag}>")).count(), "{tag}");
        }
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o644);
        assert!(!daemons.join("dev.namios.update.plist.new").exists());
        // Again: as it was.
        assert!(!set_up_launchd(&root, true).unwrap());
        // An older one: written again.
        std::fs::write(&f, text.replace("<integer>19</integer>", "<integer>10</integer>")).unwrap();
        assert!(set_up_launchd(&root, true).unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), text);
        // `updates: manual`: gone; then nothing more to do.
        assert!(set_up_launchd(&root, false).unwrap());
        assert!(!f.exists());
        assert!(!set_up_launchd(&root, false).unwrap());
        // Someone's own of that name stays.
        std::fs::write(&f, "<?xml version=\"1.0\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>dev.namios.update</string></dict></plist>\n").unwrap();
        assert!(!set_up_launchd(&root, false).unwrap());
        assert!(f.exists());
        // nami's other LaunchDaemons aren't this one's to take.
        std::fs::remove_file(&f).unwrap();
        std::fs::write(daemons.join("dev.namios.daemon.plist"), "<!-- Made by nami (install-mac.sh): installs into the store -->").unwrap();
        assert!(!set_up_launchd(&root, false).unwrap());
        assert!(daemons.join("dev.namios.daemon.plist").exists());
        // Outside a Mac's daily update, the job goes on.
        assert!(job_still_wanted());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn says_what_was_updated() {
        let apps = |n: &[&str]| n.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(said(None, &[]), None);
        assert_eq!(said(Some("nami 0.18.6"), &[]).as_deref(), Some("nami 0.18.6 is installed."));
        assert_eq!(said(None, &apps(&["firefox"])).as_deref(), Some("firefox was updated."));
        assert_eq!(said(None, &apps(&["firefox", "vlc"])).as_deref(), Some("firefox and vlc were updated."));
        assert_eq!(said(Some("nami 0.18.6"), &apps(&["a", "b", "c", "d"])).as_deref(), Some("nami 0.18.6 is installed, and 4 apps were updated."));
    }
}
