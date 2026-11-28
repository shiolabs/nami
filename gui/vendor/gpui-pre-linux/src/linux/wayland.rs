mod client;
mod clipboard;
mod cursor;
mod display;
mod popup;
mod serial;
mod window;

/// Contains Types for configuring layer_shell surfaces.
pub mod layer_shell;

pub(crate) use client::*;

/// namiOS: the layer namespaces whose surfaces hold the compositor's keys back
/// (`crate::linux::inhibit_shortcuts_in`).
pub(crate) static INHIBITING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// namiOS: the layer namespaces whose surfaces keep the screen awake while they're up
/// (`crate::linux::inhibit_idle_in`).
pub(crate) static KEEPING_AWAKE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// namiOS: the layer namespaces whose surfaces are made the session's lock surfaces instead
/// (`crate::linux::lock_session_in`).
pub(crate) static LOCKING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// namiOS: the session's lock, once a lock surface has asked for it
/// (`crate::linux::unlock_session`).
pub(crate) static SESSION_LOCK: std::sync::Mutex<
    Option<wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1>,
> = std::sync::Mutex::new(None);

/// namiOS: what the compositor's said of the lock: 0 nothing yet, 1 locked, 2 finished (it
/// wouldn't, or it's over).
pub(crate) static SESSION_LOCK_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;

use gpui::CursorStyle;

pub(super) fn to_shape(style: CursorStyle) -> Shape {
    match style {
        CursorStyle::Arrow => Shape::Default,
        CursorStyle::IBeam => Shape::Text,
        CursorStyle::Crosshair => Shape::Crosshair,
        CursorStyle::ClosedHand => Shape::Grabbing,
        CursorStyle::OpenHand => Shape::Grab,
        CursorStyle::PointingHand => Shape::Pointer,
        CursorStyle::ResizeLeft => Shape::WResize,
        CursorStyle::ResizeRight => Shape::EResize,
        CursorStyle::ResizeLeftRight => Shape::EwResize,
        CursorStyle::ResizeUp => Shape::NResize,
        CursorStyle::ResizeDown => Shape::SResize,
        CursorStyle::ResizeUpDown => Shape::NsResize,
        CursorStyle::ResizeUpLeftDownRight => Shape::NwseResize,
        CursorStyle::ResizeUpRightDownLeft => Shape::NeswResize,
        CursorStyle::ResizeColumn => Shape::ColResize,
        CursorStyle::ResizeRow => Shape::RowResize,
        CursorStyle::IBeamCursorForVerticalLayout => Shape::VerticalText,
        CursorStyle::OperationNotAllowed => Shape::NotAllowed,
        CursorStyle::DragLink => Shape::Alias,
        CursorStyle::DragCopy => Shape::Copy,
        CursorStyle::ContextualMenu => Shape::ContextMenu,
    }
}
