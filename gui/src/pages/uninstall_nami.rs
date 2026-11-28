//! Uninstall nami…: nami itself off this computer, from its window, for people who don't use a
//! terminal. On a Mac it's in the nami menu, after About nami, and on Installed; on another
//! Linux, on Installed, under everything installed. Never on namiOS, where nami is the system
//! (on namiOS's preview of another distribution it's shown, and only says what it would do).
//!
//! It asks first, over the window: that it's nami itself, not one app, what goes and what
//! stays, with Cancel (the default) and Uninstall nami. Then the system's own password prompt
//! (Linux: pkexec, as polkit asks; a Mac: osascript's administrator privileges) runs
//! `nami self-uninstall --yes`, which runs nami's uninstaller. A cancelled prompt leaves
//! everything as it was, quietly. Once it's done, the window says so and the app quits: the
//! uninstaller took it away too.

use gpui::prelude::*;
use gpui::{Context, Div, FontWeight, div, relative, rgb};

use crate::look::{Stop, color, px, text};
use crate::Shell;

/// Where Uninstall nami… is.
#[derive(Default)]
pub enum Uninstall {
    #[default]
    Idle,
    /// "Uninstall nami?", over the window.
    Asking,
    /// The password given (or being asked for): nami's uninstaller is running.
    Working,
    /// It's done: nami is gone. Quit is all that's left.
    Done,
    /// It couldn't: what nami said.
    Failed(String),
    /// namiOS's preview of another distribution: what would have happened.
    Previewed,
}

/// The title, and what's said under it, before anything's done.
pub fn asking_words(mac: bool, own_volume: bool) -> (&'static str, String, &'static str) {
    let (computer, store) = match (mac, own_volume) {
        (true, true) => ("Mac", "and the Nix Store volume it keeps them on"),
        (true, false) => ("Mac", "and its part of /nix"),
        _ => ("computer", "and its store in /nix"),
    };
    let goes = format!("This uninstalls nami itself from this {computer}, not one app: nami, this app, every app and tool installed with nami, {store}.");
    let stays = if mac {
        "Your user.nami and your home folder stay as they are. To remove one app instead, click Remove on its page."
    } else {
        "Your user.nami, system.nami and home folder stay as they are. To remove one app instead, click Remove on its page."
    };
    ("Uninstall nami?", goes, stays)
}

/// What runs it, with an administrator's password: (program, its arguments). `nami` is where
/// nami is.
pub fn command(mac: bool, nami: &str) -> (String, Vec<String>) {
    let run = format!("{nami} self-uninstall --yes");
    if mac {
        // (AppleScript's own quotes around a path with none in it: /nix/var/nami/bin/nami.)
        let script = format!("do shell script \"{}\" with prompt \"nami wants to uninstall nami from this Mac.\" with administrator privileges", run.replace('\\', "\\\\").replace('"', "\\\""));
        ("/usr/bin/osascript".into(), vec!["-e".into(), script])
    } else {
        ("pkexec".into(), vec![nami.into(), "self-uninstall".into(), "--yes".into()])
    }
}

/// Whether the password prompt was cancelled (or its password not given): then nothing's
/// changed, and nothing's said. pkexec: 126, the dialog dismissed; 127, not authorized.
/// osascript: error -128, "User canceled.".
pub fn cancelled(mac: bool, code: Option<i32>, said: &str) -> bool {
    if mac { said.contains("(-128)") || said.contains("User canceled") } else { matches!(code, Some(126 | 127)) }
}

/// nami, where it is: its own folder's, as root runs it.
pub(crate) fn nami_path(mac: bool) -> String {
    let places: &[&str] = if mac { &["/nix/var/nami/bin/nami"] } else { &["/nami/bin/nami", "/nix/var/nami/bin/nami"] };
    places.iter().find(|p| std::path::Path::new(p).is_file()).map(|p| p.to_string()).unwrap_or_else(|| crate::nami::program().to_string_lossy().into_owned())
}

/// Whether this is namiOS itself (not its preview of another distribution).
fn real_namios(g: &Shell) -> bool {
    !crate::look::MAC && (g.nami_os || std::path::Path::new("/etc/namios").exists())
}

/// Whether Uninstall nami… is offered here: wherever nami is, but namiOS.
pub fn offered(g: &Shell) -> bool {
    g.role.is_ok() && (!real_namios(g) || crate::look::as_other_linux())
}

/// Uninstall nami… chosen: it asks.
pub fn ask(g: &mut Shell, cx: &mut Context<Shell>) {
    if offered(g) && matches!(g.uninstall, Uninstall::Idle | Uninstall::Failed(_) | Uninstall::Previewed) {
        g.uninstall = Uninstall::Asking;
        cx.notify();
    }
}

/// Esc: what it's asking put away (not while it's uninstalling, or once it's done).
pub fn back(g: &mut Shell) -> bool {
    match g.uninstall {
        Uninstall::Asking | Uninstall::Failed(_) | Uninstall::Previewed => {
            g.uninstall = Uninstall::Idle;
            true
        }
        _ => false,
    }
}

/// Uninstall nami, pressed: the password, then nami's uninstaller.
fn go(g: &mut Shell, cx: &mut Context<Shell>) {
    if real_namios(g) {
        g.uninstall = Uninstall::Previewed;
        cx.notify();
        return;
    }
    g.uninstall = Uninstall::Working;
    cx.notify();
    let mac = crate::look::MAC;
    let (program, args) = command(mac, &nami_path(mac));
    Shell::later(cx, move || std::process::Command::new(&program).args(&args).stdin(std::process::Stdio::null()).output(), move |g, out, _| {
        g.uninstall = match out {
            Ok(o) if o.status.success() => Uninstall::Done,
            Ok(o) => {
                let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
                if cancelled(mac, o.status.code(), &said) { Uninstall::Idle } else { Uninstall::Failed(said.trim().to_string()) }
            }
            Err(e) => Uninstall::Failed(format!("Couldn't ask for an administrator's password ({}): {e}", if mac { "osascript" } else { "pkexec" })),
        };
    });
}

/// The row at the foot of Installed: nami itself, and the way to uninstall it.
pub fn row(g: &Shell, cx: &mut Context<Shell>) -> Option<Div> {
    if !offered(g) {
        return None;
    }
    let version = g.updates.nami.as_ref().and_then(|n| n.as_ref().ok()).map(|n| format!("Version {}", n.current));
    let busy = g.busy();
    Some(
        div().flex().flex_col().gap(px(8.)).mt(px(4.)).child(div().text_size(px(text::SECTION)).font_weight(FontWeight::SEMIBOLD).child("nami")).child(
            crate::ui::sheet().child(
                crate::ui::sheet_row(true)
                    .py(px(12.))
                    .child(crate::icons::glyph("mark", 30., color::text()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child("nami"))
                            .child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child(version.map_or_else(|| "nami itself, and every app installed with it".to_string(), |v| format!("{v} · nami itself, and every app installed with it")))),
                    )
                    .child(crate::ui::grey("uninstall-nami", "Uninstall nami…").stop(cx).when(busy, |b| b.opacity(0.45)).when(!busy, |b| b.on_click(cx.listener(|g, _, _, cx| ask(g, cx))))),
            ),
        ),
    )
}

/// The dialog over the window, while there's one: asking, uninstalling, done.
pub fn overlay(g: &Shell, cx: &mut Context<Shell>) -> Option<gpui::AnyElement> {
    use crate::look::{button, destructive_button};
    let mac = crate::look::MAC;
    let body = |t: String| div().text_size(px(text::BODY)).text_color(rgb(color::muted())).child(t);
    let title = |t: &str| div().text_size(px(text::HEADLINE)).font_weight(FontWeight::SEMIBOLD).child(t.to_string());
    let buttons = || div().flex().justify_end().gap(px(8.)).mt(px(8.));
    let card = match &g.uninstall {
        Uninstall::Idle => return None,
        Uninstall::Asking => {
            let own_volume = std::path::Path::new("/Library/LaunchDaemons/dev.namios.store.plist").exists() && !std::path::Path::new("/nix/var/nix/db/db.sqlite").exists();
            let (t, goes, stays) = asking_words(mac, own_volume);
            div()
                .child(title(t))
                .child(body(goes))
                .child(body(stays.to_string()))
                .child(
                    buttons()
                        .child(button("uninstall-cancel", "Cancel", false, true).stop(cx).on_click(cx.listener(|g, _, _, cx| {
                            g.uninstall = Uninstall::Idle;
                            cx.notify();
                        })))
                        .child(destructive_button("uninstall-go", "Uninstall nami").stop(cx).on_click(cx.listener(|g, _, _, cx| go(g, cx)))),
                )
        }
        Uninstall::Working => div()
            .child(div().flex().items_center().gap(px(10.)).child(crate::motion::spinner("uninstall-spin", 16., color::muted(), true)).child(title("Uninstalling nami…")))
            .child(body("nami and the apps installed with it are being removed. This takes a moment.".into())),
        Uninstall::Done => div()
            .child(title("nami is uninstalled"))
            .child(body(if mac { "Your user.nami and your home folder are as they were. To use nami again, install it from namios.dev." } else { "Your nami files and your home folder are as they were. To use nami again, install it from namios.dev." }.into()))
            .child(buttons().child(button("uninstall-quit", "Quit", true, true).stop(cx).on_click(|_, _, cx| cx.quit()))),
        Uninstall::Failed(said) => div()
            .child(title("nami couldn't be uninstalled"))
            .child(crate::pages::trouble("uninstall-failed", "nami's uninstaller stopped before it finished.", said.clone(), cx))
            .child(buttons().child(button("uninstall-ok", "OK", true, true).stop(cx).on_click(cx.listener(|g, _, _, cx| {
                g.uninstall = Uninstall::Idle;
                cx.notify();
            })))),
        Uninstall::Previewed => div()
            .child(title("Preview"))
            .child(body("On another distribution, nami would now be uninstalled. Nothing was removed.".into()))
            .child(buttons().child(button("uninstall-ok", "OK", true, true).stop(cx).on_click(cx.listener(|g, _, _, cx| {
                g.uninstall = Uninstall::Idle;
                cx.notify();
            })))),
    };
    Some(dialog("uninstall-ask", "Uninstall nami", card, cx))
}

/// A dialog over the window (`card`, its words and buttons), as a Mac's alerts: the window dimmed
/// under it, nothing under it reached while it's up.
pub fn dialog(id: &'static str, label: &'static str, card: Div, _cx: &mut Context<Shell>) -> gpui::AnyElement {
    div()
        .id(gpui::ElementId::Name(format!("{id}-over").into()))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(crate::look::frame_round())
        .bg(color::dim())
        .occlude()
        .child(crate::motion::enter(
            id,
            card.id(gpui::ElementId::Name(format!("{id}-card").into()))
                .w(px(440.))
                .max_w(relative(0.92))
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(24.))
                .rounded(px(crate::look::shape::CARD))
                .bg(rgb(color::panel()))
                .border_1()
                .border_color(rgb(color::line()))
                .shadow(crate::look::lifted_shadow())
                .role(gpui::Role::Dialog)
                .aria_label(label),
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_says_its_nami_itself_and_what_stays() {
        let (title, goes, stays) = asking_words(false, false);
        assert_eq!(title, "Uninstall nami?");
        assert!(goes.contains("nami itself") && goes.contains("not one app") && goes.contains("every app and tool installed with nami"), "{goes}");
        assert!(stays.contains("user.nami") && stays.contains("system.nami") && stays.contains("home folder"), "{stays}");
        let (_, goes, stays) = asking_words(true, true);
        assert!(goes.contains("from this Mac") && goes.contains("Nix Store volume"), "{goes}");
        assert!(!stays.contains("system.nami"));
        let (_, goes, _) = asking_words(true, false);
        assert!(!goes.contains("volume"), "Nix's volume stays: {goes}");
        for (a, b, c) in [asking_words(false, false), asking_words(true, true), asking_words(true, false)] {
            assert!(!format!("{a}{b}{c}").to_lowercase().contains("tray"));
        }
    }

    #[test]
    fn the_systems_own_prompt_runs_nami_self_uninstall() {
        let (p, args) = command(false, "/nami/bin/nami");
        assert_eq!((p.as_str(), args), ("pkexec", vec!["/nami/bin/nami".to_string(), "self-uninstall".into(), "--yes".into()]));
        let (p, args) = command(true, "/nix/var/nami/bin/nami");
        assert_eq!(p, "/usr/bin/osascript");
        assert_eq!(args[0], "-e");
        assert_eq!(args[1], "do shell script \"/nix/var/nami/bin/nami self-uninstall --yes\" with prompt \"nami wants to uninstall nami from this Mac.\" with administrator privileges");
    }

    #[test]
    fn a_cancelled_prompt_is_nothing() {
        assert!(cancelled(false, Some(126), ""));
        assert!(cancelled(false, Some(127), "Error executing command as another user: Not authorized"));
        assert!(!cancelled(false, Some(1), "error: nami's uninstaller isn't here"));
        assert!(cancelled(true, Some(1), "0:76: execution error: User canceled. (-128)"));
        assert!(!cancelled(true, Some(1), "error: something else (-1)"));
    }
}
