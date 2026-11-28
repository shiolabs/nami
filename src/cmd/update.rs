//! The daily update (`nami os-update`, nami-update.timer's): nami itself first, then on
//! another distribution the system's apps at their newest (on namiOS, namiOS's part makes
//! a new system for the next start: `NamiOs::daily_update`). On a Mac, each person's own
//! apps from Homebrew's casks too, by nami run as them (`nami os-update-casks`).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::casks;
use crate::index::Index;
use crate::profile::{Entry, Profiles};
use crate::store::Store;
use crate::cmd::apply::{apply_config_daily, finish, report};
use crate::cmd::generations::{clean_up_everything, say_clean_up};
use crate::{auto_update, ops, profile, selfupdate};

/// Set once `nami os-update` has seen to nami itself.
pub(crate) const NAMI_IS_FRESH: &str = "NAMI_OS_UPDATE_FRESH";

/// The nami an update started with, when it installed a newer one.
pub(crate) const NAMI_WAS: &str = "NAMI_OS_UPDATE_WAS";

/// The file a profile was last applied from, when it's been changed since (those changes
/// are its owner's to apply, not the update's).
fn not_applied(profiles: &Profiles) -> Result<Option<String>> {
    Ok(profiles.current()?.and_then(|m| m.config).filter(|snap| std::fs::read_to_string(&snap.file).ok().as_deref() != Some(snap.text.as_str())).map(|snap| snap.file))
}

/// What's in `after` at another version than in `before`, by name: an app from Homebrew's
/// casks by its own (Rectangle), anything else as the file lists it (ripgrep).
fn updated(before: &[Entry], after: &[Entry]) -> Vec<String> {
    after
        .iter()
        .filter(|e| before.iter().any(|b| b.attr == e.attr && b.name != e.name))
        .map(|e| match casks::entry_token(e) {
            Some(t) => casks::find(t).map(|s| s.name.clone()).unwrap_or_else(|| t.to_string()),
            None => e.attr.clone(),
        })
        .collect()
}

/// The daily update on another distribution, after nami itself: the newest
/// catalog (on a Mac, Homebrew's list of casks too), and the system's apps at
/// their newest (as `nami upgrade` has them, but an app that's open now),
/// which ones changed. Not while system.nami has changes not applied yet:
/// those are an admin's to apply, not the update's.
fn update_other_linux(store: &Store, profiles: &Profiles) -> Result<Vec<String>> {
    println!("catalog: {} packages", Index::refresh(store)?);
    if crate::index::MAC {
        match casks::refresh() {
            Ok(n) => println!("Homebrew's casks: {n}"),
            Err(e) => eprintln!("Homebrew's casks weren't refreshed: {e:#}"),
        }
    }
    if let Some(file) = not_applied(profiles)? {
        println!("the system's apps stay as they are: {file} has changes not applied yet");
        return Ok(Vec::new());
    }
    let index = Index::load(store)?;
    if ops::updates(profiles, &index)?.is_empty() {
        println!("the system's apps are up to date");
        return Ok(Vec::new());
    }
    let had = profiles.current_packages()?;
    let before = profiles.current_generation();
    match profiles.managed_config()? {
        Some(file) => apply_config_daily(store, profiles, &file, true)?,
        None => {
            report(profiles, before, ops::upgrade(store, profiles, &index)?)?;
            crate::tell::set_intent(crate::tell::Intent::Upgrade);
            finish(std::path::Path::new("the system's apps"), profiles);
        }
    }
    Ok(updated(&had, &profiles.current_packages()?))
}

/// `nami os-update` on another distribution: the system's apps at their newest; on a Mac,
/// then each person's apps from Homebrew's casks.
pub(crate) fn update_other_distribution(store: &Store, profiles: &Profiles) -> Result<()> {
    let daily = if crate::index::MAC { "launchd's dev.namios.update" } else { "nami-update.timer" };
    anyhow::ensure!(profile::is_root(), "nami os-update updates the computer, as root ({daily} runs it daily)");
    // Old changes first (`cleanup:`): and when space is low, the update has room.
    say_clean_up(clean_up_everything(&store));
    let system = update_other_linux(&store, &profiles);
    // (Not for Update Now, whose next step is the person's own `nami upgrade`. The store's lock
    // goes first: their changes are the daemon's to make, under it.)
    let people = if crate::index::MAC && std::env::var_os("NAMI_FOR").is_none() {
        store.release();
        update_peoples_casks(store)
    } else {
        Vec::new()
    };
    match system {
        Ok(mut apps) => {
            // (Theirs too, for whoever's at the screen: another person's are theirs alone.)
            let here = auto_update::at_the_screen();
            apps.extend(people.into_iter().filter(|(uid, _)| Some(*uid) == here).flat_map(|(_, a)| a));
            auto_update::went_well(&store, std::env::var(NAMI_WAS).ok().as_deref(), &apps);
        }
        Err(e) => {
            auto_update::went_wrong(&store);
            return Err(e);
        }
    }
    Ok(())
}

/// The people on a Mac with nami apps of their own (a profile in nami's folder): login, uid,
/// gid and home.
pub(crate) fn people_with_profiles(store: &Store) -> Vec<(String, u32, u32, PathBuf)> {
    let Ok(dirs) = std::fs::read_dir(store.var_dir().join("profiles/per-user")) else { return Vec::new() };
    let mut out: Vec<_> = dirs
        .flatten()
        .filter_map(|d| {
            let login = d.file_name().to_str()?.to_string();
            let a = crate::sys::account_by_name(&login)?;
            (a.uid >= 500 && a.name == login && a.home.is_dir()).then_some((login, a.uid, a.gid, a.home))
        })
        .collect();
    out.sort();
    out
}

/// Each person's own apps from Homebrew's casks, on a Mac, by nami run as them (`nami
/// os-update-casks`): root changes nothing in anyone's folder, their file and its lock are
/// theirs, and their nami Apps theirs. Only for whoever has one with a newer version to have.
/// Which apps changed, by whose uid.
fn update_peoples_casks(store: &Store) -> Vec<(u32, Vec<String>)> {
    use std::os::unix::process::CommandExt;
    let Ok(me) = std::env::current_exe() else { return Vec::new() };
    let mut out = Vec::new();
    for (login, uid, gid, home) in people_with_profiles(store) {
        let theirs = Profiles::user(store, &login);
        let before = theirs.current_packages().unwrap_or_default();
        if casks::updates(&before).is_empty() {
            continue;
        }
        println!("{login}'s apps from Homebrew's casks:");
        let mut c = std::process::Command::new(&me);
        c.arg("os-update-casks")
            .uid(uid)
            .gid(gid)
            .env_clear()
            .env("HOME", &home)
            .env("USER", &login)
            .env("LOGNAME", &login)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .current_dir("/")
            .stdin(std::process::Stdio::null());
        for k in ["NAMI_ROOT", "NAMI_NO_MIGRATE", "NAMI_CASKS_API"] {
            if let Some(v) = std::env::var_os(k) {
                c.env(k, v);
            }
        }
        match c.status() {
            Ok(s) if s.success() => {}
            Ok(_) => eprintln!("{login}'s apps stay as they are this time (the next update tries again)"),
            Err(e) => eprintln!("{login}'s apps: {e}"),
        }
        out.push((uid, updated(&before, &theirs.current_packages().unwrap_or_default())));
    }
    out
}

/// `nami os-update-casks`, as a person on a Mac (the daily update runs it as each one): their
/// apps from Homebrew's casks at the newest, but those that update themselves and those open
/// now. The rest of their file stays as it is, and all of it while it has changes not applied.
pub(crate) fn update_my_casks(store: &Store) -> Result<()> {
    anyhow::ensure!(crate::index::MAC && !profile::is_root(), "nami os-update-casks is the daily update's, run as each person on a Mac");
    let profiles = Profiles::personal(store)?;
    let Some(file) = profiles.managed_config()? else { return Ok(()) };
    if let Some(file) = not_applied(&profiles)? {
        println!("your apps stay as they are: {file} has changes not applied yet");
        return Ok(());
    }
    if let Err(e) = casks::refresh() {
        eprintln!("Homebrew's casks weren't refreshed: {e:#}");
    }
    if casks::updates(&profiles.current_packages()?).is_empty() {
        println!("your apps from Homebrew's casks are up to date");
        return Ok(());
    }
    apply_config_daily(store, &profiles, &file, false)
}

/// The daily update's first step: nami itself. A newer one is started in this one's
/// place for the rest; with none, the rest goes on here.
pub(crate) fn update_nami_first(root: &std::path::Path) -> Result<()> {
    let store = Store::open_direct(&root, false)?;
    // No release to be had (no network, its server down) needn't stop the rest.
    let newer = match selfupdate::check(&store).and_then(|c| selfupdate::run(&store).map(|_| c.newer)) {
        Ok(newer) => newer,
        Err(e) => {
            eprintln!("nami itself stays as it is: {e:#}");
            false
        }
    };
    drop(store);
    if newer {
        use std::os::unix::process::CommandExt;
        // (Which nami it was, to say what changed.)
        let e = std::process::Command::new(crate::migrate::var_dir(root).join("bin/nami")).arg("os-update").env(NAMI_IS_FRESH, "1").env(NAMI_WAS, selfupdate::CURRENT).exec();
        return Err(e).context("starting the new nami");
    }
    std::env::set_var(NAMI_IS_FRESH, "1");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a notice says was updated: what moved, a cask's by its token when the list's not
    /// here (unit tests have none); nothing new, gone or as it was.
    #[test]
    fn what_was_updated_is_what_moved() {
        let e = |attr: &str, name: &str| Entry { attr: attr.into(), name: name.into(), paths: vec![] };
        let before = [e("ripgrep", "ripgrep-14"), e("cask:rectangle", "cask:rectangle-0.80"), e("cask:maccy", "cask:maccy-2.0"), e("jq", "jq-1.7")];
        let after = [e("ripgrep", "ripgrep-15"), e("cask:rectangle", "cask:rectangle-0.90"), e("cask:maccy", "cask:maccy-2.0"), e("fd", "fd-10")];
        assert_eq!(updated(&before, &after), ["ripgrep", "rectangle"]);
        assert!(updated(&after, &after).is_empty());
    }
}
