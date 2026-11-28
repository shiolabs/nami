//! The nami file format itself: its version (`format: 1`) and the keys
//! with a new name. A file of its own, as nami-gui's installer edits nami
//! files too (config_edit.rs) and needs these.

/// The newest format of nami files this nami reads (`format: 1`). A file
/// without `format:` is format 1. A later nami that reads a file
/// differently writes a higher number, so this one refuses it instead of
/// misreading it.
pub const FORMAT: u64 = 1;

/// What to say when a file (or nami.lock) is from a newer nami.
pub const NEWER_NAMI: &str = "this file needs a newer nami: nami self-update";

/// Keys with a new name: (old, new). The old name still works until nami
/// 1.0, and nami says the new one.
pub const RENAMED_KEYS: &[(&str, &str)] = &[("keep-generations", "history")];
