//! nami's window on a Mac (nami.app): macOS's menu bar (nami, Edit, Window, Help), its keys
//! (Cmd+Q, Cmd+W, Cmd+M, Cmd+C...), its About panel, the Dock's click bringing the window back,
//! and nami's programs on PATH, which an app opened from Finder hasn't. The window itself is
//! macOS's, with its own buttons (look.rs's `MAC_BUTTONS`); what's in it is nami's.
//! What nami does on a Mac is in docs/macos.md.

use gpui::{App, KeyBinding, Menu, MenuItem, SystemMenuType, actions};

actions!(mac, [About, UninstallNami, Quit, Hide, HideOthers, ShowAll, Minimize, Zoom, CloseWindow, Help]);

/// Before anything else (one thread still): nami's programs first on PATH, as nami.sh has them
/// in Terminal. An app opened from Finder or the Dock has launchd's PATH (/usr/bin:/bin:...),
/// without nami, the tools it installs, or an editor's command.
pub fn prepare() {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let ours = [
        std::path::PathBuf::from("/nix/var/nami/bin"),
        std::path::PathBuf::from("/nix/var/nami/profiles/default/bin"),
        home.join(".local/state/nami/profile/bin"),
    ];
    // (And where editors and Homebrew put their commands, after the system's.)
    let after = [std::path::PathBuf::from("/usr/local/bin"), std::path::PathBuf::from("/opt/homebrew/bin")];
    let now: Vec<std::path::PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let rest = now.iter().filter(|d| !ours.contains(d) && !after.contains(d)).cloned();
    let path: Vec<std::path::PathBuf> = ours.iter().cloned().chain(rest).chain(after.iter().cloned()).collect();
    if let Ok(joined) = std::env::join_paths(path) {
        // SAFETY: first thing in main, before any other thread is started.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// The menu bar and its keys, as a Mac's apps have them. (No Settings…: nami's settings are its
/// file, ~/user.nami, on Your Setup.)
pub fn menus(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-?", Help, None),
        // A field's, as a Mac's are (Ctrl's stay, as on Linux).
        KeyBinding::new("cmd-a", crate::input::SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-c", crate::input::Copy, Some("TextInput")),
        KeyBinding::new("cmd-v", crate::input::Paste, Some("TextInput")),
        KeyBinding::new("cmd-x", crate::input::Cut, Some("TextInput")),
        KeyBinding::new("cmd-left", crate::input::Home, Some("TextInput")),
        KeyBinding::new("cmd-right", crate::input::End, Some("TextInput")),
        // And Developer mode's editor's.
        KeyBinding::new("cmd-a", crate::editor::SelectAll, Some("Editor")),
        KeyBinding::new("cmd-c", crate::editor::Copy, Some("Editor")),
        KeyBinding::new("cmd-v", crate::editor::Paste, Some("Editor")),
        KeyBinding::new("cmd-x", crate::editor::Cut, Some("Editor")),
        KeyBinding::new("cmd-z", crate::editor::Undo, Some("Editor")),
        KeyBinding::new("cmd-shift-z", crate::editor::Redo, Some("Editor")),
        KeyBinding::new("cmd-s", crate::editor::Save, Some("Editor")),
    ]);
    cx.on_action(|_: &About, _| about());
    cx.on_action(uninstall);
    cx.on_action(uninstall);
    cx.on_action(quit);
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Minimize, cx| front(cx, |w| w.minimize_window()));
    cx.on_action(|_: &Zoom, cx| front(cx, |w| w.zoom_window()));
    cx.on_action(close);
    cx.on_action(|_: &Help, cx| cx.open_url("https://namios.dev/docs/nami"));
    cx.set_menus([
        Menu::new("nami").items([
            MenuItem::action("About nami", About),
            MenuItem::action("Uninstall nami…", UninstallNami),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide nami", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit nami", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Cut", crate::input::Cut),
            MenuItem::action("Copy", crate::input::Copy),
            MenuItem::action("Paste", crate::input::Paste),
            MenuItem::action("Select All", crate::input::SelectAll),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Close", CloseWindow),
        ]),
        Menu::new("Help").items([MenuItem::action("nami Help", Help)]),
    ]);
}

/// The Dock's icon clicked with the window closed (an install still going): the window's back.
pub fn on_reopen(app: &gpui::Application) {
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            if let Some(shell) = cx.try_global::<crate::Main>().map(|m| m._held.clone()) {
                crate::open_window(shell, cx);
            }
        }
    });
}

/// The window in front, as `then` changes it.
fn front(cx: &mut App, then: impl FnOnce(&mut gpui::Window)) {
    if let Some(w) = cx.active_window().or_else(|| cx.windows().into_iter().next()) {
        let _ = w.update(cx, |_, window, _| then(window));
    }
}

/// Cmd+W, Close: the window goes, as Ctrl+W has it (not while it's changing the computer: the
/// window says so). What's being installed goes on without it; nami ends once it's done.
fn close(_: &CloseWindow, cx: &mut App) {
    let changing = cx.try_global::<crate::Main>().is_some_and(|m| m._held.read(cx).run.as_ref().is_some_and(|r| r.finished.is_none()));
    if !changing {
        front(cx, |w| w.remove_window());
    }
}

/// Cmd+Q, Quit nami: at once, unless something's being installed or changed. Then the window
/// goes, and nami ends as soon as that's done (as closing its window does), so nothing's cut off
/// halfway.
fn quit(_: &Quit, cx: &mut App) {
    let busy = cx.try_global::<crate::Main>().map(|m| m._held.read(cx).busy()).unwrap_or(false);
    if busy {
        for w in cx.windows() {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
    } else {
        cx.quit();
    }
}

/// Uninstall nami…: the window (back, if it was closed), asking first (pages/uninstall_nami.rs).
fn uninstall(_: &UninstallNami, cx: &mut App) {
    let Some(shell) = cx.try_global::<crate::Main>().map(|m| m._held.clone()) else { return };
    if cx.windows().is_empty() {
        crate::open_window(shell.clone(), cx);
    }
    shell.update(cx, |g, cx| crate::pages::uninstall_nami::ask(g, cx));
}

/// macOS's About panel: nami's name, mark and version, from nami.app's Info.plist.
fn about() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    app.orderFrontStandardAboutPanel(None);
}
