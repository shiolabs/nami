//! What goes through the daemon: which commands change the computer, and running them
//! as root for an admin (or with an admin's password).

use std::path::PathBuf;

use anyhow::Result;

use crate::profile::Profiles;
use crate::store::Store;
use crate::cli::{Cli, Command};
use crate::cmd::files::{and_list, base_of, is_system_config, personal_config};
use crate::cmd::packages::{install_split, remove_split};
use crate::cmd::apply::layers_file;
use crate::{bundle_app, daemon, flathub, profile, store, style};

/// Does `command` change the system's apps (so an admin runs it as root
/// through the daemon)?
pub(crate) fn changes_system(command: &Command) -> bool {
    match command {
        // Flathub's apps are each person's own: never the computer's.
        Command::Install { packages, .. } | Command::Remove { packages, .. } if !packages.is_empty() && packages.iter().all(|p| flathub::is_flathub(p) || bundle_app::is_entry(p)) => false,
        // Everyone's apps when asked for; each person's own otherwise.
        Command::Install { system, .. } | Command::Remove { system, .. } => *system,
        Command::Rollback { system: true, .. } | Command::Gc { dry_run: false, auto: false, mine: false, .. } | Command::SystemInit { .. } | Command::SystemUse { .. } => true,
        Command::Upgrade { dry_run, system, .. } => !dry_run && *system,
        Command::OsUpdate { set_up } => !set_up,
        Command::SelfUpdate { check, .. } => !check,
        Command::Update { from, .. } => from.is_some(),
        // Your file, unless it's named, or --system says the system's.
        Command::Apply { file, system, .. } => file.as_ref().map_or(*system, |f| is_system_config(f)),
        Command::Layers { file, update: true, .. } => is_system_config(&layers_file(file.clone())),
        _ => false,
    }
}

/// An install or removal by someone (not root): what's theirs stays here,
/// and an administrator's for everyone goes to the daemon, which runs it as
/// root (`--system`). What's left is the command to go on with.
pub(crate) fn for_whom(cli: Cli, root: &std::path::Path) -> Result<Cli> {
    if profile::is_root() {
        return Ok(cli);
    }
    let admin = profile::is_admin();
    let (everyone, mine, verb, json) = match cli.command {
        // Someone who isn't an admin, for everyone: the daemon asks for an admin's name and
        // password (Flathub's stay theirs).
        Command::Install { packages, system: true, .. } if !admin && !packages.is_empty() => {
            let (everyone, mine) = install_split(packages, true);
            (everyone, mine, "install", false)
        }
        Command::Install { packages, system, json } if !packages.is_empty() => {
            let (everyone, mut mine) = install_split(packages, system);
            if !admin {
                // As before: theirs, with the rest of what it needs still to come.
                if !everyone.is_empty() && !profile::is_child() {
                    eprintln!("note: {} {} up the computer (a service, files in /etc): it works fully once an administrator installs it for the system (nami install --system {})", and_list(&everyone), if everyone.len() == 1 { "sets" } else { "set" }, everyone.join(" "));
                }
                mine.extend(everyone);
                return Ok(Cli { color: style::Choice::Auto, command: Command::Install { packages: mine, system: false, json } });
            }
            if !system && !everyone.is_empty() {
                eprintln!("{} {} up the computer: installing for everyone on this computer", and_list(&everyone), if everyone.len() == 1 { "sets" } else { "set" });
            }
            (everyone, mine, "install", json)
        }
        Command::Remove { packages, system: false, json } if admin && !packages.is_empty() => {
            // (Your file as last applied: ~/user.nami, or one in a config repo.)
            let applied = Store::open(root).and_then(|s| Profiles::personal(&s)?.managed_config()).ok().flatten();
            let file = applied.unwrap_or_else(personal_config);
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let (everyone, mine) = remove_split(packages, &text, &base_of(&file));
            (everyone, mine, "remove", json)
        }
        Command::Remove { packages, system: true, json } => (packages, Vec::new(), "remove", json),
        other => return Ok(Cli { color: style::Choice::Auto, command: other }),
    };
    if !everyone.is_empty() {
        let mut args = vec![verb.to_string(), "--system".to_string()];
        // (Its {"ok": true} said there, when nothing's left to do here.)
        if json && mine.is_empty() {
            args.push("--json".to_string());
        }
        args.extend(everyone);
        let code = daemon::client::run(&store::daemon_socket(root), &args, &std::env::current_dir()?)?;
        if code != 0 || mine.is_empty() {
            std::process::exit(code);
        }
    }
    Ok(Cli {
        color: style::Choice::Auto,
        command: match verb {
            "install" => Command::Install { packages: mine, system: false, json },
            _ => Command::Remove { packages: mine, system: false, json },
        },
    })
}

/// Whether a command changes one's own apps (a child's are the computer's).
pub(crate) fn changes_own_apps(command: &Command) -> bool {
    match command {
        Command::Install { .. } | Command::Remove { .. } | Command::Rollback { .. } => true,
        Command::Upgrade { dry_run, .. } => !dry_run,
        _ => false,
    }
}

/// nami's own program, as it is now (to start again for something).
pub(crate) fn daemon_program() -> PathBuf {
    std::env::current_exe().ok().map(|p| match p.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(s) if std::path::Path::new(s).exists() => PathBuf::from(s),
        _ => p,
    }).unwrap_or_else(|| PathBuf::from("nami"))
}

/// Admins change the system's apps without sudo: the daemon runs the
/// command as root for them (it checks they're in wheel/sudo/admin).
/// Someone who isn't an admin, naming system.nami to apply, or installing updates
/// (Update Now): the daemon too, which asks for an admin's name and password.
pub(crate) fn forward(command: &Command, root: &std::path::Path) -> Result<()> {
    let unlocked = !profile::is_admin() && (matches!(command, Command::Apply { file: Some(f), .. } if is_system_config(f)) || matches!(command, Command::Apply { system: true, .. } | Command::Rollback { system: true, .. } | Command::Upgrade { system: true, dry_run: false, .. } | Command::OsUpdate { set_up: false }));
    if !profile::is_root() && (profile::is_admin() || unlocked) && changes_system(command) {
        let socket = store::daemon_socket(root);
        let args: Vec<String> = std::env::args().skip(1).collect();
        let code = daemon::client::run(&socket, &args, &std::env::current_dir()?)?;
        std::process::exit(code);
    }
    Ok(())
}
