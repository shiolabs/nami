//! nami-gui: nami with a window. Your apps and setup (~/user.nami), the
//! computer's apps for admins (system.nami), every step to go back to, and
//! what the system adds (os.rs: on namiOS, its Settings). It's the
//! nami command underneath: the files are the truth, changed in place
//! (comments kept), and each change is put in place as it's made (with an
//! admin's password, for everyone's), as a Mac's Settings does it. Only in
//! Developer mode, where the files are edited as text, do changes wait for Apply.

// A few of the shared parts' functions are namiOS's alone (nami.rs's backups and
// installing, some of look.rs and ui.rs), unused here.
#![allow(dead_code)]

mod bounded;
#[cfg(unix)]
#[allow(dead_code)]
mod clock;
mod editor;
#[path = "../../src/file_diff.rs"]
mod file_diff;
mod file_lines;
mod icons;
#[allow(dead_code)]
mod themes;
mod input;
mod installs;
mod jobs;
mod launch;
mod look;
mod tip;
mod menu;
/// What a menu does to be light or dark: the window's already said it.
pub fn menu_theme(_: &mut gpui::Window, _: &mut gpui::App) {}
mod pages;
// The system nami-gui is for (os.rs): nami's own window, or namiOS's Settings too.
mod os;
use os::{Active, Os};
mod pictures;
mod nami;
// nami.app's: macOS's menu bar and keys, nami's programs on PATH.
#[cfg(target_os = "macos")]
mod mac;
mod fit;
mod motion;
mod levels;
mod picker;
mod progress;
mod ui;
mod vim;
// The namiOS Installer's own (gui/src/stick): the latest release, the USB drives, the
// writer and what it hands namiOS, for Move to namiOS (pages/move_to.rs).
#[path = "stick/release.rs"]
#[allow(dead_code)]
mod release;
#[path = "stick/disks.rs"]
#[allow(dead_code)]
mod disks;
#[path = "stick/seed.rs"]
#[allow(dead_code)]
mod seed;
#[path = "stick/gpt.rs"]
mod gpt;
#[path = "stick/write.rs"]
#[allow(dead_code)]
mod write;
mod file_view;
// Settings' state (the files, the changes, the runs) and, in shell/view.rs, its window's
// chrome: the sidebar, the toolbar, the page shown.
mod shell;

use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{App, Bounds, Entity, KeyBinding, Window, WindowBounds, WindowOptions, actions, size};
use crate::look::px;
use serde_json::Value;

pub use shell::{CHANGING, Page, Problem, Run, Shell, Taking, Undo, Undoing};
pub(crate) use shell::desktop_part;
use shell::{FIRST_LOOK, look_up};

actions!(gui, [FocusNext, FocusPrev]);

/// The page the desktop asked for, if it asked (Super+Comma, the launcher, the control center).
fn asked_page() -> Option<String> {
    // (A preview, as on another distribution, leaves what's asked of Settings to Settings.)
    if look::as_other_linux() {
        return None;
    }
    let file = std::path::Path::new(&std::env::var_os("XDG_RUNTIME_DIR")?).join("nami-gui-settings");
    let text = std::fs::read_to_string(&file).ok()?;
    std::fs::remove_file(&file).ok()?;
    Some(text)
}

/// ~/user.nami, read here: quick, for how the window looks from its first frame (nami's own
/// reading of it follows).
fn own_file() -> Value {
    let text = std::fs::read_to_string(crate::nami::user_file()).ok();
    text.and_then(|t| crate::nami::yaml(&t).ok()).and_then(|v| serde_json::to_value(v).ok()).unwrap_or(Value::Null)
}

/// How frosted this person has the desktop (look::Glass::read: the old `blur` too), as
/// nami-desktop reads it.
fn glass_of(user: &Value) -> crate::look::Glass {
    let look = &user["namios"]["appearance"];
    crate::look::Glass::read(look["glass"].as_str(), look["blur"].as_str(), look["reduce-transparency"].as_bool() == Some(true), look["theme"].as_str() == Some(themes::HIGH_CONTRAST))
}

/// The colours this person picked (namiOS's desktop), the size it's all drawn at, and whether
/// anything moves: this window's too.
fn follow_look(user: &Value, cx: &mut App) {
    let look = &user["namios"]["appearance"];
    themes::apply(look["theme"].as_str(), look["accent"].as_str());
    crate::look::set_glass(glass_of(user));
    crate::look::set_scale(crate::look::scale_of(look["text-size"].as_str()));
    cx.set_reduce_motion(look["animations"].as_bool() == Some(false));
    crate::motion::set_reduced(look["reduce-motion"].as_bool() == Some(true));
}

/// Asked of niri on its socket (no `niri msg` to start): its answer, if it gives one soon.
fn niri(request: Value) -> Option<Value> {
    use std::io::{BufRead, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(std::env::var_os("NIRI_SOCKET")?).ok()?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(500))).ok()?;
    stream.set_write_timeout(Some(std::time::Duration::from_millis(500))).ok()?;
    stream.write_all(format!("{request}\n").as_bytes()).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// Settings' window, if it's open. (Only niri can say; elsewhere each start is a window, as ever.)
fn already_open() -> Option<u64> {
    let answer = niri(serde_json::json!("Windows"))?;
    answer["Ok"]["Windows"].as_array()?.iter().find(|w| w["app_id"] == "nami-gui" && w["title"] == "Settings")?["id"].as_u64()
}

/// Settings is one window: whether this is it. Started again, even while the first is still
/// opening its window, the one there is comes forward (at the page asked for, if any) and
/// this ends. (A lock held as long as it runs, as niri sees no window until it's drawn.)
fn the_only_settings() -> bool {
    static HELD: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    // (Only niri can bring it forward; elsewhere each start is a window, as ever.)
    let (Some(dir), Some(_)) = (std::env::var_os("XDG_RUNTIME_DIR"), std::env::var_os("NIRI_SOCKET")) else { return true };
    // (As on another distribution, a preview: a window of its own, next to Settings.)
    if look::as_other_linux() {
        return true;
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(Path::new(&dir).join("nami-gui-settings.lock")) else { return true };
    if file.try_lock().is_ok() {
        let _ = HELD.set(file);
        return true;
    }
    // An app from the launcher: the one running takes it, as it is, window or none.
    if install_app().is_some() {
        return false;
    }
    let asked = if let Some(f) = open_file_arg() {
        Some(format!("file:{}", f.display()))
    } else if std::env::args().any(|a| a == "--install") {
        Some("install".into())
    } else if std::env::args().any(|a| a == "--apps") {
        Some("apps".into())
    } else {
        // (`--page <name>`: the notification of a request, say.)
        std::env::args().skip_while(|a| a != "--page").nth(1)
    };
    let file = Path::new(&dir).join("nami-gui-settings");
    match asked {
        Some(page) => {
            let _ = std::fs::write(&file, page);
        }
        // Only its window, then (the one running may have none): unless the desktop's asked
        // for a page already.
        None => {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).open(&file) {
                let _ = f.write_all(b"show");
            }
        }
    }
    // (The first may still be opening: its window, once it's there.)
    for _ in 0..40 {
        if let Some(id) = already_open() {
            niri(serde_json::json!({"Action": {"FocusWindow": {"id": id}}}));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}

/// `--open-file <path>`: a download to install (a .deb, .rpm, AppImage or
/// tarball opened in Files: `nami open-package`).
fn open_file_arg() -> Option<PathBuf> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let at = args.iter().position(|a| a == "--open-file")?;
    let p = PathBuf::from(args.get(at + 1)?);
    Some(std::fs::canonicalize(&p).unwrap_or(p))
}

/// `--install-app <attr>`: from the launcher, to install that app without a window.
fn install_app() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == "--install-app").and_then(|i| args.get(i + 1).cloned())
}

/// Settings, kept as long as it runs: its window can close while an app goes on.
struct Main {
    _held: Entity<Shell>,
}
impl gpui::Global for Main {}

/// Settings' window, for the one Settings there is.
fn open_window(shell: Entity<Shell>, cx: &mut App) {
    // As tall as the sidebar wants, on a small screen too: the whole screen less the bar's room;
    // never taller than niri lets it be (its rule's max-height), so it's drawn at the size it
    // keeps, in its place, from the first frame.
    let screen = cx.primary_display().map(|d| d.bounds().size.height.as_f32()).unwrap_or(800.);
    // (Framed by itself, the room for its shadow about it too.)
    let room = if look::own_frame() { 2. * look::FRAME_INSET } else { 0. };
    let bounds = Bounds::centered(None, size(px(980. + room), gpui::px((760. * crate::look::scale()).min(780.).min(screen - 56.).max(520.) + room)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some(look::app_id().into()),
            titlebar: Some(if look::MAC {
                // A Mac's window: macOS's frame and buttons, over nami's sidebar (as a Mac's
                // System Settings has them), its title bar see-through.
                gpui::TitlebarOptions { title: Some("nami".into()), appears_transparent: true, traffic_light_position: Some(gpui::point(gpui::px(look::MAC_BUTTONS.0), gpui::px(look::MAC_BUTTONS.1))) }
            } else {
                gpui::TitlebarOptions { title: Some(if look::as_other_linux() { "Settings (as on Other Distributions)" } else if look::own_frame() { "nami" } else { "Settings" }.into()), ..Default::default() }
            }),
            // (Framed by itself, round on its shadow: what's outside its corners shows through.)
            window_decorations: look::decorations(),
            window_background: if look::own_frame() { gpui::WindowBackgroundAppearance::Transparent } else { gpui::WindowBackgroundAppearance::Opaque },
            ..Default::default()
        },
        |window, cx| {
            follow_theme(window);
            window.observe_window_appearance(|window, _| follow_theme(window)).detach();
            shell.update(cx, |_, cx| {
                cx.observe_window_activation(window, |this, window, cx| {
                    if window.is_window_active() {
                        this.look_again(cx);
                    }
                })
                .detach();
            });
            shell
        },
    )
    .unwrap_or_else(|e| no_window(&e.to_string()));
    cx.activate(true);
}

fn main() {
    // (On a Mac, nami's programs on PATH first: started from Finder, it has launchd's.)
    #[cfg(target_os = "macos")]
    mac::prepare();
    // `--version`: nami's, which this window comes with (a release sets NAMI_VERSION).
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("nami-gui {}", option_env!("NAMI_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")));
        return;
    }
    look::set_type();
    // (Its fields in its groups' family.)
    input::set_inset();
    // (The system's own commands: namiOS's `nami-gui privacy …` and the like.)
    if Active::command(&std::env::args().skip(1).collect::<Vec<_>>()) {
        return;
    }
    if pages::move_to::command(&std::env::args().skip(1).collect::<Vec<_>>()) {
        return;
    }
    if let Some(attr) = install_app() {
        if let Err(e) = installs::ask(&attr) {
            eprintln!("can't ask for {attr}: {e}");
            std::process::exit(1);
        }
    }
    // (A program of the system's instead of the window: namiOS's timeline of the backups.)
    if Active::run_apart() {
        return;
    }
    // (One of the system's own windows: namiOS's welcome, Install namiOS.)
    let own_window = Active::own_window_asked();
    if !own_window && !the_only_settings() {
        return;
    }
    // (Asked now, beside gpui's start: it's known by the time the window is.)
    std::thread::spawn(|| *DESKTOP_DARK);
    // Who this is and their files (nami, a few times over: a moment), beside gpui's start too.
    if !own_window {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(look_up()));
        if let Ok(mut f) = FIRST_LOOK.lock() {
            *f = Some(rx);
        }
    }
    let app = gpui_platform::application().with_assets(icons::Assets);
    #[cfg(target_os = "macos")]
    mac::on_reopen(&app);
    app.run(move |cx: &mut App| {
        // (A Mac's menu bar, and its keys.)
        #[cfg(target_os = "macos")]
        mac::menus(cx);
        // Apps' pictures made sharp for their size: drawn again with them as they come.
        cx.spawn(async move |cx| {
            let mut seen = pictures::made();
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                let now = pictures::made();
                if now != seen {
                    seen = now;
                    let _ = cx.update(|cx| cx.refresh_windows());
                }
            }
        })
        .detach();
        // Geist, and Geist Mono for files: the same on
        // every system (the usual variable fonts lose their bold in gpui).
        let fonts = [&include_bytes!("../fonts/Geist-Regular.ttf")[..], include_bytes!("../fonts/Geist-Medium.ttf"), include_bytes!("../fonts/Geist-SemiBold.ttf"), include_bytes!("../fonts/Geist-Bold.ttf"), include_bytes!("../fonts/GeistMono-Regular.otf")];
        cx.text_system().add_fonts(fonts.into_iter().map(std::borrow::Cow::Borrowed).collect()).expect("nami-gui's fonts");
        input::bind_keys(cx);
        editor::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("tab", FocusNext, None), KeyBinding::new("shift-tab", FocusPrev, None)]);
        cx.on_window_closed(|cx, _| {
            // (Settings itself ends once what it's doing is done.)
            if cx.windows().is_empty() && !cx.has_global::<Main>() {
                cx.quit();
            }
        })
        .detach();
        if own_window && Active::open_own_window(cx) {
            return;
        }
        let shell = cx.new(Shell::new);
        shell.update(cx, |shell, cx| shell.take_first_look(cx));
        cx.set_global(Main { _held: shell.clone() });
        // For an app from the launcher: no window, unless one's asked for.
        if install_app().is_none() {
            open_window(shell.clone(), cx);
        }
        if std::env::args().any(|a| a == "--smoke-test") {
            smoke_test(shell, cx);
        }
    });
}

/// `--smoke-test` (CI's, on each Mac): the window opens, nami's first look is taken (who this
/// is, the files), a frame is drawn after it, and the program ends saying so. It changes
/// nothing. Not done in 60 seconds: it says so and fails.
fn smoke_test(shell: Entity<Shell>, cx: &mut App) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DREW: AtomicBool = AtomicBool::new(false);
    let started = std::time::Instant::now();
    cx.spawn(async move |cx| {
        let mut asked = false;
        loop {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            let done = cx.update(|cx| {
                let looked = shell.read(cx).first_looked();
                let window = cx.windows().into_iter().next();
                if let (Some((page, nami)), Some(window)) = (looked, window) {
                    if DREW.load(Ordering::Relaxed) {
                        if let Err(why) = letters(cx) {
                            eprintln!("nami-gui: the window drew, but no letters: {why}");
                            std::process::exit(1);
                        }
                        println!("nami-gui: the window opened and drew, at {page:?} (nami answered: {nami})");
                        cx.quit();
                        return true;
                    }
                    if !asked {
                        asked = true;
                        let _ = window.update(cx, |_, window, _| {
                            window.on_next_frame(|_, _| DREW.store(true, Ordering::Relaxed));
                            window.refresh();
                        });
                    }
                }
                if started.elapsed() > std::time::Duration::from_secs(60) {
                    eprintln!("nami-gui: the window didn't open and draw in 60 seconds");
                    std::process::exit(1);
                }
                false
            });
            if done {
                return;
            }
        }
    })
    .detach();
}

/// The smoke test's check that the window's text is drawn: the text system is a real one, with
/// Geist and Geist Mono in it, and gives Geist's letters their own widths. (gpui's macOS part,
/// built without its `font-kit` feature, has none: every letter's left out, and its stand-in
/// still lays text out, every letter as wide as the next. nami.app 0.21.0 shipped so.)
fn letters(cx: &App) -> Result<(), String> {
    let text = cx.text_system();
    let names = text.all_font_names();
    for family in ["Geist", "Geist Mono"] {
        if !names.iter().any(|n| n == family) {
            return Err(format!("the text system has no {family} ({} fonts in it)", names.len()));
        }
    }
    let geist = text.resolve_font(&gpui::font("Geist"));
    let width = |c| text.advance(geist, px(13.), c).map(|a| f32::from(a.width)).map_err(|e| e.to_string());
    let (i, w) = (width('i')?, width('W')?);
    if !(i > 0. && w > i * 2.) {
        return Err(format!("Geist's i and W are {i} and {w} wide"));
    }
    Ok(())
}

/// The window couldn't open (gpui draws with Vulkan: no driver, not even
/// mesa's software one, or no display): say so where it'll be seen, the
/// terminal and a notification, since started from the app menu nothing shows.
fn no_window(why: &str) -> ! {
    // (A Mac's window draws with Metal, which every Mac nami runs on has.)
    if look::MAC {
        eprintln!("nami's window couldn't open: {why}. Use `nami --help` in Terminal to do the same things there.");
        std::process::exit(1)
    }
    let say = format!("nami's window couldn't open: {why}. The window needs Vulkan. Run `nami doctor` to check the graphics drivers, or use `nami --help` to do the same things from the terminal.");
    eprintln!("{say}");
    let _ = std::process::Command::new("notify-send").args(["--app-name=nami", "nami's window couldn't open", &say]).status();
    std::process::exit(1)
}

/// Light or dark, as the window opens. gpui takes it to be light until the desktop's portal
/// has answered (over D-Bus, after the window's up): a dark desktop's Settings, welcome and
/// timeline would draw their first frames light, the white flash niri's open animation then
/// grows from. So it's asked first, as gpui will be told: by then the window is right.
fn follow_theme(window: &mut Window) {
    use std::sync::atomic::{AtomicBool, Ordering};
    // Whether gpui has said dark yet: its light is only its own until then. (With the portal
    // just started, it says light a few times as the window opens, then dark a second later.)
    static HEARD_DARK: AtomicBool = AtomicBool::new(false);
    let says_dark = matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
    if says_dark {
        HEARD_DARK.store(true, Ordering::Relaxed);
    }
    let dark = match std::env::var("NAMI_THEME").as_deref() {
        Ok("dark") => true,
        Ok("light") => false,
        _ if says_dark || HEARD_DARK.load(Ordering::Relaxed) => says_dark,
        // Light from gpui before it's ever said dark: as the desktop is, if it's dark. (Asked
        // again past the first time: they may have just turned it light.)
        _ => match *DESKTOP_DARK {
            Some(true) if ui::color::DARK.load(Ordering::Relaxed) => desktop_dark_now().unwrap_or(true),
            Some(dark) => dark,
            None => false,
        },
    };
    ui::color::DARK.store(dark, Ordering::Relaxed);
    window.refresh();
}

/// GNOME's colour scheme, read now.
fn desktop_dark_now() -> Option<bool> {
    gnome_scheme_dark(&crate::bounded::text("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"], std::time::Duration::from_millis(500)))
}

/// Whether the desktop is dark, asked once: GNOME's colour scheme, which namiOS's desktop sets
/// and the portal passes on to gpui (read from dconf: a few milliseconds, where the portal,
/// just after logging in, is started by the asking and takes a third of a second). Without it,
/// the portal's `org.freedesktop.appearance color-scheme` (1 dark, 2 light), if it answers in a
/// moment.
static DESKTOP_DARK: std::sync::LazyLock<Option<bool>> = std::sync::LazyLock::new(|| {
    // (A Mac's window knows its appearance as it opens: nothing to ask.)
    if look::MAC {
        return None;
    }
    if let Some(dark) = desktop_dark_now() {
        return Some(dark);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ask = || -> Option<bool> {
            let conn = zbus::blocking::Connection::session().ok()?;
            let reply = conn.call_method(Some("org.freedesktop.portal.Desktop"), "/org/freedesktop/portal/desktop", Some("org.freedesktop.portal.Settings"), "ReadOne", &("org.freedesktop.appearance", "color-scheme")).ok()?;
            let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
            color_scheme_dark(&value)
        };
        let _ = tx.send(ask());
    });
    rx.recv_timeout(std::time::Duration::from_millis(400)).ok().flatten()
});

/// GNOME's `color-scheme` as gsettings prints it: 'prefer-dark', 'prefer-light', or 'default'
/// (light, as the portal says it).
fn gnome_scheme_dark(said: &str) -> Option<bool> {
    match said.trim().trim_matches('\'') {
        "prefer-dark" => Some(true),
        "prefer-light" | "default" => Some(false),
        _ => None,
    }
}

/// The portal's colour scheme (a u32, sometimes in a variant of its own): 1 is dark, 2 light,
/// 0 no preference (nothing said).
fn color_scheme_dark(value: &zbus::zvariant::Value) -> Option<bool> {
    match value {
        zbus::zvariant::Value::Value(inner) => color_scheme_dark(inner),
        zbus::zvariant::Value::U32(1) => Some(true),
        zbus::zvariant::Value::U32(2) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
#[test]
fn the_portals_dark_is_one() {
    use zbus::zvariant::Value;
    assert_eq!(color_scheme_dark(&Value::U32(1)), Some(true));
    assert_eq!(color_scheme_dark(&Value::U32(2)), Some(false));
    // (A portal just started, before its GNOME part's up: no answer, so GNOME's setting is asked.)
    assert_eq!(color_scheme_dark(&Value::U32(0)), None);
    assert_eq!(color_scheme_dark(&Value::Value(Box::new(Value::U32(1)))), Some(true));
    assert_eq!(gnome_scheme_dark("'prefer-dark'\n"), Some(true));
    assert_eq!(gnome_scheme_dark("'default'"), Some(false));
    assert_eq!(gnome_scheme_dark(""), None);
}
