mod dispatcher;
mod display_connection;
mod headless;
mod keyboard;
mod platform;
mod system_notifications;
#[cfg(any(feature = "wayland", feature = "x11"))]
mod text_system;
#[cfg(feature = "wayland")]
mod wayland;
#[cfg(feature = "x11")]
mod x11;

#[cfg(any(feature = "wayland", feature = "x11"))]
mod xdg_desktop_portal;

pub use dispatcher::*;
pub(crate) use display_connection::{Backend, DisplayConnection, select_backend};
pub(crate) use headless::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) use text_system::*;
#[cfg(feature = "wayland")]
pub(crate) use wayland::*;
#[cfg(feature = "x11")]
pub(crate) use x11::*;

use std::rc::Rc;

/// namiOS: the display's connection is gone (the compositor let go of this program, or went
/// itself). Nothing this program shows is there any more, and it can't show anything again:
/// it ends at once, with status 1, rather than going on with nowhere to draw (and nothing torn
/// down that would wait for the display: a window's surface, the GPU's swapchain). What
/// started it can start it again (namiOS's desktop: niri's loop does, while niri's there).
pub(crate) fn display_lost(error: &dyn std::fmt::Display) -> ! {
    log::error!("the display's connection is gone: {error}");
    eprintln!("{}: the display's connection is gone ({error}); ending", std::env::args().next().unwrap_or_default());
    std::process::exit(1)
}

/// namiOS: the compositor's own keys held back while a layer surface of this namespace has
/// the keyboard (zwp_keyboard_shortcuts_inhibit_manager_v1), so they come to it: for
/// nami-desktop's Alt+Tab, held with Super, whose arrows niri has for itself otherwise. It
/// asks as the surface is made; the asking goes with the surface.
#[cfg(feature = "wayland")]
pub fn inhibit_shortcuts_in(namespace: &str) {
    wayland::INHIBITING.lock().unwrap_or_else(|e| e.into_inner()).push(namespace.to_string());
}

/// namiOS: the screen kept from going off, dimming or locking by itself while a layer surface
/// of this namespace is up (zwp_idle_inhibit_manager_v1, which niri honours for what watches
/// for idle, swayidle): for nami-desktop's password box, so it isn't asked into a dark screen.
/// It asks as the surface is made; the asking goes with the surface.
#[cfg(feature = "wayland")]
pub fn inhibit_idle_in(namespace: &str) {
    wayland::KEEPING_AWAKE.lock().unwrap_or_else(|e| e.into_inner()).push(namespace.to_string());
}

/// namiOS: layer surfaces of this namespace are the session's lock surfaces instead
/// (ext_session_lock_v1), one for each screen (a window opened on each display): the first
/// asks for the lock. Until `unlock_session`, the compositor shows nothing else, and keeps the
/// screen locked even if this program ends.
#[cfg(feature = "wayland")]
pub fn lock_session_in(namespace: &str) {
    wayland::LOCKING.lock().unwrap_or_else(|e| e.into_inner()).push(namespace.to_string());
}

/// namiOS: what the compositor has said of the session's lock.
#[cfg(feature = "wayland")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionLock {
    /// Asked for, not yet said (or not asked yet).
    Waiting,
    /// The screen's locked: nothing but the lock surfaces is shown.
    Locked,
    /// It wouldn't lock (another program has), or it's over.
    Finished,
}

#[cfg(feature = "wayland")]
pub fn session_lock() -> SessionLock {
    match wayland::SESSION_LOCK_STATE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => SessionLock::Locked,
        2 => SessionLock::Finished,
        _ => SessionLock::Waiting,
    }
}

/// namiOS: the session unlocked (once it's said it's locked), and the lock's surfaces gone
/// with it; sent at once. Whether it was.
#[cfg(feature = "wayland")]
pub fn unlock_session() -> bool {
    use wayland_client::Proxy;
    if session_lock() != SessionLock::Locked {
        return false;
    }
    let Some(lock) = wayland::SESSION_LOCK.lock().unwrap_or_else(|e| e.into_inner()).take() else { return false };
    lock.unlock_and_destroy();
    wayland::SESSION_LOCK_STATE.store(2, std::sync::atomic::Ordering::SeqCst);
    lock.backend().upgrade().is_some_and(|backend| wayland_client::Connection::from_backend(backend).flush().is_ok())
}

/// namiOS: a key of the keyboard as the compositor's keymap has it, in the layout in use (what
/// nami-desktop's Keyboard Shortcuts draws): its evdev code, the name of the keysym it types
/// alone (`q`, `slash`, `adiaeresis`, `Super_L`; for a layout that isn't Latin, the Latin
/// layout's, which is what the compositor's shortcuts go by), and what it types, as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCap {
    pub code: u32,
    pub name: String,
    pub text: String,
}

/// namiOS: the layout in use's name, and its keys, once the compositor has given its keymap.
pub(crate) static KEYBOARD: std::sync::Mutex<Option<(String, Vec<KeyCap>)>> = std::sync::Mutex::new(None);

/// namiOS: the keyboard as it is now: the layout's name ("Danish") and its keys; None before
/// the compositor has said (or under X11).
pub fn keyboard_keys() -> Option<(String, Vec<KeyCap>)> {
    KEYBOARD.lock().ok()?.clone()
}

/// namiOS: noted as the keymap comes, and as the layout in use changes.
#[allow(dead_code)]
pub(crate) fn note_keyboard(state: &xkbcommon::xkb::State) {
    use xkbcommon::xkb;
    let layout = state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE);
    let keymap = state.get_keymap();
    let layouts = keymap.num_layouts();
    let level0 = |code: u32, layout: u32| keymap.key_get_syms_by_level(xkb::Keycode::new(code), layout, 0).first().copied().filter(|s| s.raw() != 0);
    let mut keys = Vec::new();
    for code in 8u32..=255 {
        let Some(sym) = level0(code, layout) else { continue };
        let text: String = xkb::keysym_to_utf8(sym).chars().filter(|c| !c.is_control()).collect();
        // (A letter not in Latin: the compositor's shortcuts go by the first Latin layout's.)
        let latin = (sym.raw() > 0xff && text.chars().any(char::is_alphabetic))
            .then(|| (0..layouts).filter_map(|l| level0(code, l)).find(|s| s.raw() <= 0xff))
            .flatten();
        keys.push(KeyCap { code: code - 8, name: xkb::keysym_get_name(latin.unwrap_or(sym)), text });
    }
    if let Ok(mut kept) = KEYBOARD.lock() {
        *kept = Some((keymap.layout_get_name(layout).to_string(), keys));
    }
}

use gpui::WindowingModes;

/// Returns the default platform implementation for the current OS.
///
/// A windowed platform connects to the display server the process environment names, or starts
/// headless when it names none, and can switch between those modes later. A headless platform
/// stays headless.
pub fn current_platform(headless: bool) -> Rc<dyn gpui::Platform> {
    if headless || std::env::var_os("ZED_HEADLESS").is_some() {
        linux_platform(WindowingModes::HEADLESS)
    } else {
        linux_platform(WindowingModes::all())
    }
}

/// Returns a platform that may switch among `allowed_modes`.
///
/// It starts windowed in the process's own environment, or headless if that names no allowed
/// display server. Set another initial mode with [`gpui::Application::with_windowing`].
pub fn linux_platform(allowed_modes: WindowingModes) -> Rc<dyn gpui::Platform> {
    Rc::new(LinuxPlatform::new(allowed_modes))
}
