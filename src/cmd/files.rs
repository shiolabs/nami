//! Which nami file a command means: the computer's (system.nami, for root and
//! admins) or the person's own (user.nami), and the folder each is in.

use std::path::PathBuf;

use anyhow::Result;

use crate::profile::Profiles;
use crate::cmd::setup::{share_with_admins, system_dir};
use crate::{config, profile};

/// The file a command means when it isn't named: sudo decides. Root's is the
/// system file (/etc/nami/system.nami), anyone else's their own, admin or not
/// (`--system` names the system file for programs with no terminal for sudo).
/// Never a file in the current folder: a project.nami is a project's, and any
/// other is named (`nami apply ./user.nami`).
pub(crate) fn default_config() -> PathBuf {
    if profile::is_root() {
        return system_config();
    }
    personal_config()
}

/// The system file: the system's apps, for everyone who uses it.
pub(crate) fn system_config() -> PathBuf {
    config::system_file()
}

pub(crate) fn is_system_config(file: &std::path::Path) -> bool {
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(file) == canon(&system_config())
}

/// The user file: your home settings (env, aliases, dotfiles), and the
/// apps of someone who isn't an admin.
pub(crate) fn personal_config() -> PathBuf {
    config::user_file()
}

/// "docker", "docker and steam".
pub(crate) fn and_list(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// What a child is told when they'd change their own apps.
pub(crate) const CHILD_APPS: &str = "you're a child on this computer: an admin adds and removes your apps (on the computer, in its system.nami)";

/// The computer's file, created (from what's installed) if it isn't
/// there yet: /etc/nami belongs to root and the admin group, so admins can
/// edit it without sudo. Root only.
pub(crate) fn ensure_system_config(profiles: &Profiles) -> Result<PathBuf> {
    let file = system_config();
    if file.exists() {
        return Ok(file);
    }
    let dir = &system_dir(&config::hostname().unwrap_or_else(|| "computer".into()))?;
    let text = match profiles.current()? {
        Some(m) => config::export(&m),
        None => "packages:\n".into(),
    };
    std::fs::write(&file, format!("# The system's apps, for everyone who uses this computer. Admins can change this file;\n# `sudo nami apply` makes the computer match. Your own settings: ~/user.nami\n{}", text.lines().filter(|l| !l.starts_with("# Generated")).collect::<Vec<_>>().join("\n") + "\n"))?;
    share_with_admins(dir, 0o2755)?;
    profile::for_admins(&file)?;
    println!("wrote {}", file.display());
    Ok(file)
}

/// ~/user.nami, written from what you have installed when there's none yet,
/// so the apps you install are in it from the first one.
pub(crate) fn ensure_user_config(profiles: &Profiles) -> Result<PathBuf> {
    let file = personal_config();
    if file.exists() {
        return Ok(file);
    }
    let text = match profiles.current()? {
        Some(m) => config::export(&m),
        None => "packages:\n".into(),
    };
    std::fs::write(&file, format!("# Your apps and settings; `nami apply` makes your profile match.\n{}", text.lines().filter(|l| !l.starts_with("# Generated")).collect::<Vec<_>>().join("\n") + "\n"))?;
    println!("wrote {}", file.display());
    Ok(file)
}

pub(crate) fn base_of(file: &std::path::Path) -> PathBuf {
    // A linked file (~/user.nami into a config repo): the repo's folder.
    let real = std::fs::read_link(file).ok().map(|t| file.parent().unwrap_or(std::path::Path::new(".")).join(t));
    real.as_deref().unwrap_or(file).parent().filter(|p| !p.as_os_str().is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
