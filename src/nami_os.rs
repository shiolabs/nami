//! Where namiOS plugs into nami: the one place the rest of nami asks about it.
//!
//! namiOS is a whole computer that nami makes from a system.nami (`nami usb`, `nami iso`)
//! and then keeps up to date: `sudo nami apply` makes it a new system, and its history is
//! its systems. That part of nami is in namios/, built in when the folder is there (build.rs:
//! `cfg(namios)`); a nami built without it is nami on any other distribution, and says so
//! when asked for namiOS's commands.
//!
//! The rule: whether this computer is namiOS is `here()`, from what's on its disk;
//! doing namiOS's work is `plugin()`'s.

use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::profile::Profiles;
use crate::query::Step;
use crate::store::Store;

/// Is this computer namiOS (one `nami usb` or the namiOS Installer made)? From its disk,
/// whichever nami asks.
pub fn here() -> bool {
    Path::new("/run/booted-system/activate").exists() && has_systems(Path::new("/"))
}

/// Whether the computer at `root` has namiOS's systems (each a whole computer, made by
/// `nami usb` or `sudo nami apply`).
pub fn has_systems(root: &Path) -> bool {
    crate::migrate::var_dir(root).join("os").is_dir()
}

/// namiOS's part of nami, when it's built in.
#[cfg(namios)]
pub fn plugin() -> Option<&'static dyn NamiOs> {
    Some(&crate::namios::NAMI_OS)
}

/// (Built without it, as the public repo's nami is.)
#[cfg(not(namios))]
pub fn plugin() -> Option<&'static dyn NamiOs> {
    None
}

/// What a nami without namiOS's part says on namiOS, before it changes anything.
pub const BUILT_WITHOUT: &str = "This nami was built without namiOS support, so it can't change this computer. Use /nami/bin/nami instead.";

/// namiOS's part, for what changes a namiOS computer.
pub fn require() -> Result<&'static dyn NamiOs> {
    plugin().ok_or_else(|| anyhow::anyhow!("{BUILT_WITHOUT}"))
}

/// On namiOS, a nami built without namiOS's part changes nothing as root: the computer is
/// namiOS's systems, which only that part knows (what they use, how they update).
pub fn refuse_without() -> Result<()> {
    refuses(plugin().is_some(), crate::profile::is_root(), here())
}

fn refuses(built_in: bool, root: bool, on_namios: bool) -> Result<()> {
    anyhow::ensure!(built_in || !root || !on_namios, "{BUILT_WITHOUT}");
    Ok(())
}

/// namiOS's commands that people (and nami-gui) type.
const COMMANDS: &[&str] = &["usb", "iso", "try-os", "install-to"];

/// What a nami built without namiOS's part says to one of namiOS's commands (`args`: the
/// whole command line), instead of not knowing it.
pub fn without(args: &[String]) -> Option<String> {
    without_in(args, plugin().is_some())
}

fn without_in(args: &[String], built_in: bool) -> Option<String> {
    if built_in {
        return None;
    }
    let mut words = args.iter().skip(1);
    let name = loop {
        match words.next()?.as_str() {
            "--color" => {
                words.next();
            }
            w if w.starts_with('-') => {}
            w => break w,
        }
    };
    COMMANDS.contains(&name).then(|| format!("`nami {name}` needs namiOS support, and this nami was built without it. The nami from namios.dev/install includes it."))
}

/// system.nami was just written, as root: namiOS follows what it does at once.
pub fn system_file_changed(before: &str, after: &str) {
    if let Some(os) = plugin() {
        os.system_file_changed(before, after);
    }
}

/// What namiOS does that the rest of nami asks for. Each is called once for a command.
pub trait NamiOs: Sync {
    // The command line.

    /// nami's commands (`cmd`), with namiOS's added.
    fn commands(&self, cmd: clap::Command) -> clap::Command;
    /// Whether the command `name` is one of namiOS's.
    fn owns(&self, name: &str) -> bool;
    /// Run one of namiOS's commands (`m`: the whole command line's), at the point in
    /// main.rs's `run()` where nami runs its own: after the computer's places are nami's,
    /// before the daemon and the store's lock (each takes them as it needs).
    fn run(&self, root: &Path, m: &clap::ArgMatches) -> Result<()>;
    /// Its commands in `nami help`, first in the namiOS group.
    fn help_group(&self) -> &'static [&'static str];
    /// Examples under its commands' `--help`.
    fn examples(&self) -> &'static [(&'static str, &'static [(&'static str, &'static str)])];
    /// Whether the daemon runs `args` as root for `name` (`uid`), who needn't be an admin
    /// (namiOS's login screen, setting up the first person). None: not namiOS's to say.
    fn daemon_allows(&self, args: &[String], uid: u32, name: &str) -> Option<Result<()>>;

    // The computer.

    /// `sudo nami apply` of system.nami: the system it describes, made and switched to.
    fn switch(&self, store: &Store, file: &Path, config: &Config, dry_run: bool) -> Result<()>;
    /// The daily update (`nami os-update`), after nami itself: a new system for the next
    /// start, if anything changed.
    fn daily_update(&self, store: &Store) -> Result<()>;
    /// system.nami was just written (`nami config`, a proposal): what namiOS follows at
    /// once (the time zone).
    fn system_file_changed(&self, before: &str, after: &str);
    /// Started from its USB drive, not a disk of its own.
    fn started_from_stick(&self) -> bool;

    // Its systems are the computer's history.

    /// `nami history` of the computer: its systems, newest first.
    fn history(&self, root: &Path) -> Result<Vec<Step>>;
    /// `nami diff` of the computer's changes.
    fn diff(&self, from: Option<u64>, to: Option<u64>, raw: bool, json: bool) -> Result<()>;
    /// `nami undo` of the computer's: the apps of that system again, through system.nami.
    fn undo(&self, store: &Store, profiles: &Profiles, generation: Option<u64>) -> Result<()>;
    /// The running system: its flat file's text, and its store paths.
    fn current_system(&self, root: &Path) -> Option<(String, Vec<String>)>;
    /// Every system's store paths but `except`'s: what the store's cleanup keeps.
    fn roots(&self, root: &Path, except: &[u64]) -> Vec<String>;
    /// The systems `policy` would remove (`nami gc`); never the running one, the one
    /// before it, or what starts next.
    fn pick(&self, root: &Path, policy: &crate::profile::Prune) -> Vec<u64>;
    /// Remove those systems, with their boot entries: which went.
    fn remove(&self, root: &Path, systems: &[u64]) -> Vec<u64>;
    /// The old systems `cleanup:` says go (`low`: the disk is nearly full): which went.
    fn clean_up(&self, root: &Path, policy: &crate::cleanup::Policy, now: u64, low: bool) -> Vec<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On namiOS, a nami without namiOS's part refuses to run as root; anywhere else, or as
    /// someone else, it's nami as on any distribution.
    #[test]
    fn without_namios_nothing_changes_namios() {
        assert_eq!(refuses(false, true, true).unwrap_err().to_string(), BUILT_WITHOUT);
        assert!(refuses(true, true, true).is_ok());
        assert!(refuses(false, false, true).is_ok());
        assert!(refuses(false, true, false).is_ok());
        assert!(BUILT_WITHOUT.contains("/nami/bin/nami"));
    }

    /// `nami usb` and the rest of namiOS's, from a nami without it: said, plainly.
    #[test]
    fn namios_commands_without_namios_are_said() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let said = without_in(&args(&["nami", "usb", "--list"]), false).unwrap();
        assert_eq!(said, "`nami usb` needs namiOS support, and this nami was built without it. The nami from namios.dev/install includes it.");
        assert!(without_in(&args(&["nami", "--color", "never", "install-to", "--list"]), false).unwrap().starts_with("`nami install-to`"));
        assert!(without_in(&args(&["nami", "--json", "iso"]), false).is_some());
        assert!(without_in(&args(&["nami", "usb"]), true).is_none());
        assert!(without_in(&args(&["nami", "install", "usb"]), false).is_none());
        assert!(without_in(&args(&["nami"]), false).is_none());
        // (`nami try` is nami's own: a command without installing it.)
        assert!(without_in(&args(&["nami", "try-os", "./laptop"]), false).unwrap().starts_with("`nami try-os`"));
        assert!(without_in(&args(&["nami", "try", "cowsay"]), false).is_none());
    }
}
