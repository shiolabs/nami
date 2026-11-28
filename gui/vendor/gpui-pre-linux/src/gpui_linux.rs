#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{current_platform, linux_platform};
#[cfg(feature = "wayland")]
pub use linux::{SessionLock, inhibit_idle_in, inhibit_shortcuts_in, lock_session_in, session_lock, unlock_session};
// namiOS: the keyboard as the compositor has it (nami-desktop's Keyboard Shortcuts).
pub use linux::{KeyCap, keyboard_keys};
