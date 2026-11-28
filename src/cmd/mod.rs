//! nami's commands, a file for each group. main.rs parses the command line
//! (src/cli.rs) and sends each command to its function here; what's left in
//! main.rs's `run` is which commands need the store, the daemon or root.

pub(crate) mod files;
pub(crate) mod packages;
pub(crate) mod apply;
pub(crate) mod config;
pub(crate) mod generations;
pub(crate) mod project;
pub(crate) mod bundle;
pub(crate) mod update;
pub(crate) mod setup;
pub(crate) mod daemon;
pub(crate) mod propose;
pub(crate) mod catalog;
pub(crate) mod desktop;
pub(crate) mod misc;
pub(crate) mod self_uninstall;
