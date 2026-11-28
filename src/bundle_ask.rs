//! What a bundle with a window asks, and says, as it first starts: "Add
//! Myapp to your apps?" with Add and Not Now, once, and who made it. The
//! runtime has only std, so it asks with what the desktop has: a
//! notification with buttons (org.freedesktop.Notifications, through gdbus
//! or busctl), else a dialog (zenity, kdialog), else nothing. It's asked
//! from a process of its own: the app starts at once whatever happens.
//!
//! The answer is kept in `~/.config/nami-bundle/asked/<app>`.

// Shared by the two runtimes (nami doesn't use it).
#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEST: &str = "org.freedesktop.Notifications";
const OBJECT: &str = "/org/freedesktop/Notifications";
/// How long a question waits for its answer.
const WAIT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Answer {
    Add,
    NotNow,
    /// Nobody answered (it timed out), or it couldn't be asked.
    Nothing,
}

/// What's known of where it starts, for the decision.
#[derive(Debug, Clone, Default)]
pub struct Here {
    /// The bundle has a window (a desktop file).
    pub gui: bool,
    /// A display to show the question on.
    pub display: bool,
    /// It's in the app menu already (its own entry, or namiOS's).
    pub in_menu: bool,
    /// The person answered before.
    pub answered: bool,
    /// NAMI_BUNDLE_ASK=0, or it's started by nami (installing it).
    pub quiet: bool,
    /// Its key isn't the one trusted for it: never offer to add it.
    pub key_changed: bool,
}

/// Whether to ask "Add Myapp to your apps?" as it starts.
pub fn should_ask(h: &Here) -> bool {
    h.gui && h.display && !h.in_menu && !h.answered && !h.quiet && !h.key_changed
}

pub fn display() -> bool {
    ["WAYLAND_DISPLAY", "DISPLAY"].iter().any(|v| std::env::var_os(v).is_some_and(|v| !v.is_empty()))
}

pub fn asked_file(app: &str) -> PathBuf {
    crate::bundle_trust::dir().with_file_name("asked").join(app)
}

pub fn answered(app: &str) -> bool {
    asked_file(app).exists()
}

pub fn remember(app: &str, a: Answer) {
    let word = match a {
        Answer::Add => "add",
        Answer::NotNow => "not now",
        Answer::Nothing => return,
    };
    let f = asked_file(app);
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    let _ = std::fs::write(f, format!("{word}\n"));
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|f| f.is_file())
}

/// A GVariant string literal, for gdbus.
fn gv(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The notification server can show buttons.
fn has_actions_gdbus(gdbus: &Path) -> bool {
    let out = Command::new(gdbus).args(["call", "--session", "--dest", DEST, "--object-path", OBJECT, "--method", "org.freedesktop.Notifications.GetCapabilities"]).stdin(Stdio::null()).stderr(Stdio::null()).output();
    out.is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("'actions'"))
}

/// `(uint32 7,)` -> 7.
fn notification_id(out: &str) -> Option<u32> {
    let t = out.trim().trim_start_matches('(').trim_start_matches("uint32").trim();
    t.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// What a monitor's line says of notification `id`: an answer, its end, or nothing.
pub fn signal_line(line: &str, id: u32) -> Option<Answer> {
    if line.contains("ActionInvoked") && line.contains(&format!("uint32 {id},")) {
        return Some(if line.contains("'add'") { Answer::Add } else { Answer::NotNow });
    }
    if line.contains("NotificationClosed") && line.contains(&format!("uint32 {id},")) {
        // Closed by the person (2) is Not Now; expired (1) or closed by a call (3) is no answer.
        return Some(if line.contains("uint32 2)") { Answer::NotNow } else { Answer::Nothing });
    }
    None
}

/// busctl's JSON lines: (member, id, action).
pub fn busctl_line(line: &str, id: u32) -> Option<Answer> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let member = v.get("member")?.as_str()?;
    let data = v.get("payload")?.get("data")?.as_array()?;
    if data.first()?.as_u64()? != id as u64 {
        return None;
    }
    match member {
        "ActionInvoked" => Some(if data.get(1).and_then(|a| a.as_str()) == Some("add") { Answer::Add } else { Answer::NotNow }),
        "NotificationClosed" => Some(if data.get(1).and_then(|r| r.as_u64()) == Some(2) { Answer::NotNow } else { Answer::Nothing }),
        _ => None,
    }
}

/// Ask with a notification's buttons through gdbus.
fn ask_gdbus(gdbus: &Path, app: &str, icon: &str, title: &str, body: &str, yes: &str, no: &str) -> Option<Answer> {
    if !has_actions_gdbus(gdbus) {
        return None;
    }
    let mut mon = Command::new(gdbus).args(["monitor", "--session", "--dest", DEST, "--object-path", OBJECT]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut lines = BufReader::new(mon.stdout.take()?).lines();
    // Its first line says it's listening.
    let _ = lines.next();
    let out = Command::new(gdbus)
        .args(["call", "--session", "--dest", DEST, "--object-path", OBJECT, "--method", "org.freedesktop.Notifications.Notify"])
        .args([gv(app), "0".into(), gv(icon), gv(title), gv(body), format!("[{}, {}, {}, {}]", gv("add"), gv(yes), gv("later"), gv(no)), "{'resident': <true>}".into(), "0".into()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let Some(id) = notification_id(&String::from_utf8_lossy(&out.stdout)) else {
        let _ = mon.kill();
        return None;
    };
    let start = Instant::now();
    let mut answer = Answer::Nothing;
    // (Lines come as they're said; the monitor is killed after the wait.)
    let killer = mon.id();
    std::thread::spawn(move || {
        std::thread::sleep(WAIT);
        // SAFETY: a signal to the monitor we started.
        unsafe { libc::kill(killer as i32, libc::SIGTERM) };
    });
    for line in lines.map_while(Result::ok) {
        if let Some(a) = signal_line(&line, id) {
            answer = a;
            break;
        }
        if start.elapsed() > WAIT {
            break;
        }
    }
    let _ = mon.kill();
    let _ = mon.wait();
    if answer != Answer::Add {
        let _ = Command::new(gdbus).args(["call", "--session", "--dest", DEST, "--object-path", OBJECT, "--method", "org.freedesktop.Notifications.CloseNotification", &id.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    Some(answer)
}

/// The same through systemd's busctl.
fn ask_busctl(busctl: &Path, app: &str, icon: &str, title: &str, body: &str, yes: &str, no: &str) -> Option<Answer> {
    let caps = Command::new(busctl).args(["--user", "call", DEST, OBJECT, DEST, "GetCapabilities"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !String::from_utf8_lossy(&caps.stdout).contains("\"actions\"") {
        return None;
    }
    let mut mon = Command::new(busctl).args(["--user", "--json=short", "monitor", DEST]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    std::thread::sleep(Duration::from_millis(300));
    let out = Command::new(busctl)
        .args(["--user", "call", DEST, OBJECT, DEST, "Notify", "susssasa{sv}i", app, "0", icon, title, body, "4", "add", yes, "later", no, "1", "resident", "b", "true", "0"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    // "u 7"
    let id: u32 = String::from_utf8_lossy(&out.stdout).trim().strip_prefix("u ").and_then(|n| n.trim().parse().ok())?;
    let killer = mon.id();
    std::thread::spawn(move || {
        std::thread::sleep(WAIT);
        // SAFETY: a signal to the monitor we started.
        unsafe { libc::kill(killer as i32, libc::SIGTERM) };
    });
    let mut answer = Answer::Nothing;
    for line in BufReader::new(mon.stdout.take()?).lines().map_while(Result::ok) {
        if let Some(a) = busctl_line(&line, id) {
            answer = a;
            break;
        }
    }
    let _ = mon.kill();
    let _ = mon.wait();
    Some(answer)
}

/// A dialog: zenity's or kdialog's.
fn ask_dialog(app: &str, title: &str, body: &str, yes: &str, no: &str) -> Option<Answer> {
    let text = format!("{title}\n\n{body}");
    let status = if let Some(z) = which("zenity") {
        Command::new(z).args(["--question", "--title", app, "--text", &text, "--ok-label", yes, "--cancel-label", no, "--no-wrap"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().ok()?
    } else if let Some(k) = which("kdialog") {
        Command::new(k).args(["--title", app, "--yes-label", yes, "--no-label", no, "--yesno", &text]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().ok()?
    } else {
        return None;
    };
    Some(match status.code() {
        Some(0) => Answer::Add,
        Some(1) => Answer::NotNow,
        _ => Answer::Nothing,
    })
}

/// Ask the question with whatever the desktop has (None: nothing could).
pub fn ask(app: &str, icon: &str, title: &str, body: &str, yes: &str, no: &str) -> Option<Answer> {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() || std::env::var_os("XDG_RUNTIME_DIR").is_some() {
        if let Some(g) = which("gdbus") {
            if let Some(a) = ask_gdbus(&g, app, icon, title, body, yes, no) {
                return Some(a);
            }
        } else if let Some(b) = which("busctl") {
            if let Some(a) = ask_busctl(&b, app, icon, title, body, yes, no) {
                return Some(a);
            }
        }
    }
    ask_dialog(app, title, body, yes, no)
}

/// Say something once, without a question: a notification, else on the
/// terminal (where there is one), else nothing.
pub fn tell(app: &str, icon: &str, title: &str, body: &str) {
    use std::io::IsTerminal;
    let null = Stdio::null;
    // On the terminal it was started from; else as a notification, where there's a desktop to show one.
    if std::io::stderr().is_terminal() || !display() || which("gdbus").is_none() && which("notify-send").is_none() {
        eprintln!("{app}: {title} {body}");
    } else if let Some(g) = which("gdbus") {
        let _ = Command::new(g)
            .args(["call", "--session", "--dest", DEST, "--object-path", OBJECT, "--method", "org.freedesktop.Notifications.Notify"])
            .args([gv(app), "0".into(), gv(icon), gv(title), gv(body), "[]".into(), "{}".into(), "-1".into()])
            .stdin(null())
            .stdout(null())
            .stderr(null())
            .status();
    } else if let Some(n) = which("notify-send") {
        let _ = Command::new(n).args(["--app-name", app, "--icon", icon, title, body]).stdin(null()).stdout(null()).stderr(null()).status();
    }
}

/// Said once per app (`key`: what was said): whether it's new.
pub fn first_time(app: &str, what: &str) -> bool {
    let f = crate::bundle_trust::dir().with_file_name("told").join(format!("{app}-{what}"));
    if f.exists() {
        return false;
    }
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    std::fs::write(f, "").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_only_an_app_with_a_window_that_isnt_in_the_menu() {
        let h = Here { gui: true, display: true, ..Default::default() };
        assert!(should_ask(&h));
        assert!(!should_ask(&Here { gui: false, ..h.clone() }), "a command");
        assert!(!should_ask(&Here { display: false, ..h.clone() }), "no display");
        assert!(!should_ask(&Here { in_menu: true, ..h.clone() }), "in the menu already");
        assert!(!should_ask(&Here { answered: true, ..h.clone() }), "asked before");
        assert!(!should_ask(&Here { quiet: true, ..h.clone() }));
        assert!(!should_ask(&Here { key_changed: true, ..h }));
    }

    #[test]
    fn reads_the_answers() {
        assert_eq!(notification_id("(uint32 17,)\n"), Some(17));
        assert_eq!(signal_line("/org/freedesktop/Notifications: org.freedesktop.Notifications.ActionInvoked (uint32 17, 'add')", 17), Some(Answer::Add));
        assert_eq!(signal_line("/org/freedesktop/Notifications: org.freedesktop.Notifications.ActionInvoked (uint32 17, 'later')", 17), Some(Answer::NotNow));
        assert_eq!(signal_line("/org/freedesktop/Notifications: org.freedesktop.Notifications.ActionInvoked (uint32 3, 'add')", 17), None, "another notification");
        assert_eq!(signal_line("/org/freedesktop/Notifications: org.freedesktop.Notifications.NotificationClosed (uint32 17, uint32 2)", 17), Some(Answer::NotNow));
        assert_eq!(signal_line("/org/freedesktop/Notifications: org.freedesktop.Notifications.NotificationClosed (uint32 17, uint32 1)", 17), Some(Answer::Nothing));
        assert_eq!(busctl_line(r#"{"type":"signal","member":"ActionInvoked","payload":{"type":"us","data":[5,"add"]}}"#, 5), Some(Answer::Add));
        assert_eq!(busctl_line(r#"{"type":"signal","member":"NotificationClosed","payload":{"type":"uu","data":[5,2]}}"#, 5), Some(Answer::NotNow));
        assert_eq!(busctl_line(r#"{"type":"signal","member":"ActionInvoked","payload":{"type":"us","data":[6,"add"]}}"#, 5), None);
        assert_eq!(gv("Ada's \\ app"), "'Ada\\'s \\\\ app'");
    }
}
