//! Looking back: `nami history`, `diff`, `undo` and `gc`, and the cleanup of old changes.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::profile::Profiles;
use crate::store::Store;
use crate::cli::Command;
use crate::cmd::files::personal_config;
use crate::{cleanup, config, config_edit, file_diff, lock, log, migrate, ops, profile, store, style, tell};

/// The daily cleanup, as root (`nami gc --auto`, and the daily update first of all): old
/// changes as `cleanup:` says, the computer's (on namiOS its systems, and their boot
/// entries) and each person's (as them), then the store's paths nothing uses, at low
/// priority. What it did, as a line (none: nothing went).
pub(crate) fn clean_up_everything(store: &Store) -> Result<Option<String>> {
    // (Not when someone's waiting for it: Update Now, through the daemon.)
    if std::env::var_os("NAMI_FOR").is_none() {
        cleanup::low_priority();
    }
    let root = store.root();
    let now = cleanup::now();
    let low = cleanup::space_low(&store.phys(store::STORE_DIR));
    let policy = cleanup::of_file(&config::system_file());
    let mut done = cleanup::Done::default();
    let computer = Profiles::system(store).clean_up(&policy, now, low)?;
    done.computer = if crate::nami_os::here() { crate::nami_os::require()?.clean_up(root, &policy, now, low) } else { computer };
    done.people = clean_up_people(store);
    // What nothing uses now; not what's just come (a person's install, not in their step yet).
    (done.paths, done.bytes) = store.gc_with(&profile::all_roots(store)?, &store::Sweep { spare_newer_than: Some(3600), progress: false })?;
    log::settle();
    Ok(done.line(low))
}

/// What the daily cleanup did, said in a line (the daily update's log); nothing if nothing went.
pub(crate) fn say_clean_up(done: Result<Option<String>>) {
    match done {
        Ok(Some(line)) => println!("{}", style::out().line(&line)),
        Ok(None) => {}
        Err(e) => eprintln!("{}", style::err().line(&format!("warning: old changes stay for now: {e:#}"))),
    }
}

/// Each person's old changes, as their own file says: by nami run as them (root changes
/// nothing in anyone's folder). Who, and which went.
fn clean_up_people(store: &Store) -> Vec<(String, Vec<u64>)> {
    use std::os::unix::process::CommandExt;
    let Ok(me) = std::env::current_exe() else { return Vec::new() };
    let per_user = store.var_dir().join("profiles/per-user");
    let mut out = Vec::new();
    // (A Mac's people aren't in /etc/passwd: those with a profile in nami's folder.)
    let people = if crate::index::MAC { crate::cmd::update::people_with_profiles(store) } else { profile::people() };
    for (login, uid, gid, home) in people {
        if !per_user.join(&login).is_dir() {
            continue;
        }
        let mut c = std::process::Command::new(&me);
        c.args(["gc", "--auto", "--json"])
            .uid(uid)
            .gid(gid)
            .env_clear()
            .env("HOME", &home)
            .env("USER", &login)
            .env("LOGNAME", &login)
            .env("PATH", "/run/current-system/sw/bin:/usr/bin:/bin")
            .current_dir("/")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        for k in ["NAMI_ROOT", "NAMI_NOW", "NAMI_SPACE_LOW"] {
            if let Some(v) = std::env::var_os(k) {
                c.env(k, v);
            }
        }
        let Ok(mut child) = c.spawn() else { continue };
        // (A minute at most: it only deletes a few folders.)
        let started = std::time::Instant::now();
        while matches!(child.try_wait(), Ok(None)) && started.elapsed() < std::time::Duration::from_secs(60) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        }
        let mut said = String::new();
        let _ = child.stdout.take().map(|mut o| std::io::Read::read_to_string(&mut o, &mut said));
        let gone: Vec<u64> = serde_json::from_str::<serde_json::Value>(said.trim()).ok().and_then(|v| serde_json::from_value(v["changes"].clone()).ok()).unwrap_or_default();
        if !gone.is_empty() {
            out.push((login, gone));
        }
    }
    out
}

/// Your own `cleanup:`: your file as it is (not there, as your current change had it).
pub(crate) fn my_cleanup(mine: &Profiles) -> cleanup::Policy {
    match std::fs::read_to_string(personal_config()) {
        Ok(text) => cleanup::of_text(&text).unwrap_or_default(),
        Err(_) => mine.current().ok().flatten().and_then(|m| m.config).and_then(|c| cleanup::of_text(&c.text)).unwrap_or_default(),
    }
}

/// Your own old changes cleaned up, as your `cleanup:` says (noted, for the next day's).
fn clean_up_mine(store: &Store, low: bool) -> Result<Vec<u64>> {
    let mine = Profiles::personal(store)?;
    let gone = mine.clean_up(&my_cleanup(&mine), cleanup::now(), low)?;
    let _ = std::fs::write(mine.dir().join(CLEANED), b"");
    Ok(gone)
}

/// How many of the newest changes `cleanup:` keeps, for `nami gc --now`: the computer's
/// file says it for the system's changes, your own for yours.
pub(crate) fn cleanup_keep(profiles: &Profiles) -> usize {
    if profiles.is_system() { cleanup::of_file(&config::system_file()).keep } else { my_cleanup(profiles).keep }
}

/// Beside a person's steps: when their own were last cleaned up.
const CLEANED: &str = "cleaned";

/// `nami gc --auto`: as root, everyone's (the daily cleanup); as anyone else, their own.
pub(crate) fn gc_auto(root: &std::path::Path, json: bool) -> Result<()> {
    if profile::is_root() {
        let store = Store::open_direct(root, true)?;
        let line = clean_up_everything(&store)?;
        if json {
            println!("{}", serde_json::json!({ "said": line }));
        } else if let Some(l) = line {
            println!("{}", style::out().line(&l));
        }
        return Ok(());
    }
    let store = Store::open_direct_readonly(root)?;
    let low = cleanup::space_low(&store.phys(store::STORE_DIR));
    let gone = clean_up_mine(&store, low)?;
    if json {
        println!("{}", serde_json::json!({ "changes": gone }));
    } else {
        let done = cleanup::Done { computer: gone, ..Default::default() };
        if let Some(l) = done.line(low) {
            println!("{}", style::out().line(&l));
        }
    }
    Ok(())
}

/// The commands a person's daily cleanup goes with: those that use their apps or steps
/// (not what runs at every prompt, or as they type).
pub(crate) fn cleans_up_mine(command: &Command) -> bool {
    matches!(command, Command::Install { .. } | Command::Remove { .. } | Command::Apply { dry_run: false, .. } | Command::Upgrade { dry_run: false, .. } | Command::List { .. } | Command::Generations { .. } | Command::Rollback { .. } | Command::Status { .. } | Command::FileApps { update: true, .. } | Command::Home { .. })
}

/// A person's own old changes cleaned up, once a day as they use nami (`cleanup:`); quietly,
/// unless the disk is nearly full.
pub(crate) fn clean_up_mine_daily(root: &std::path::Path) {
    let Some(user) = std::env::var("USER").ok().filter(|u| !u.is_empty() && !u.contains('/') && !u.starts_with('.')) else { return };
    let dir = migrate::var_dir(root).join("profiles/per-user").join(&user);
    let fresh = std::fs::metadata(dir.join(CLEANED)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e.as_secs() < 86400);
    if !dir.is_dir() || fresh {
        return;
    }
    let Ok(store) = Store::open_direct_readonly(root) else { return };
    let low = cleanup::space_low(&store.phys(store::STORE_DIR));
    if let Ok(gone) = clean_up_mine(&store, low) {
        let done = cleanup::Done { computer: gone, ..Default::default() };
        if let (true, Some(l)) = (low, done.line(low)) {
            eprintln!("{}", style::err().dim(&l));
        }
    }
}

/// `nami apply` with the disk nearly full: old changes go first, by `cleanup:` (the file
/// being applied says it) but whatever their age, and as root then what nothing uses. Said
/// in a line.
pub(crate) fn clean_up_early(store: &Store, profiles: &Profiles, config: &Config) {
    let policy = config.cleanup.unwrap_or_default();
    let done = (|| -> Result<Option<String>> {
        let now = cleanup::now();
        let mut done = cleanup::Done { computer: profiles.clean_up(&policy, now, true)?, ..Default::default() };
        if profiles.is_system() && profile::is_root() {
            if crate::nami_os::here() {
                done.computer = crate::nami_os::require()?.clean_up(store.root(), &policy, now, true);
            }
            (done.paths, done.bytes) = store.gc_with(&profile::all_roots(store)?, &store::Sweep { spare_newer_than: Some(3600), progress: false })?;
        }
        Ok(done.line(true))
    })();
    match done {
        Ok(Some(line)) => eprintln!("{}", style::err().line(&line)),
        Ok(None) => {}
        Err(e) => eprintln!("{}", style::err().line(&format!("warning: space is low, and old changes couldn't be cleaned up: {e:#}"))),
    }
}

/// `nami gc --dry-run`.
pub(crate) fn gc_dry_run(root: &std::path::Path, old: bool, keep: Option<usize>, older_than: Option<String>, now: bool, mine: bool, json: bool) -> Result<()> {
    let store = Store::open_direct_readonly(&root)?;
    profile::reading_only();
    // An admin's is the computer's (as `nami gc` through the daemon); else (or with --user) yours.
    let profiles = if mine { Profiles::personal(&store)? } else if profile::is_root() || profile::is_admin() { Profiles::system(&store) } else { Profiles::for_current_user(&store)? };
    let policy = profile::Prune { all: old, keep: if now { Some(cleanup_keep(&profiles)) } else { keep }, newer_than: older_than.as_deref().map(profile::parse_duration).transpose()? };
    let mut steps = profiles.pick(&policy)?;
    // On namiOS the system's changes are its systems.
    let systems = if profiles.is_system() && crate::nami_os::here() { crate::nami_os::require()?.pick(&root, &policy) } else { Vec::new() };
    // Unused downloads go only when it's the computer's (root).
    let (paths, bytes) = if profiles.is_system() { crate::storage::unused(&store, &profile::all_roots_without(&store, &[(&profiles, &steps)], &systems)?)? } else { (0, 0) };
    if crate::nami_os::here() && profiles.is_system() {
        steps = systems;
    }
    if json {
        println!("{}", serde_json::json!({ "changes": steps, "steps": steps, "paths": paths, "bytes": bytes, "keep": policy.keep }));
    } else {
        let list: Vec<String> = steps.iter().map(u64::to_string).collect();
        let mut said = vec!["Dry run".to_string(), format!("would free {}", tell::size(bytes)), format!("{} no longer used", tell::count(paths as usize, "package", "packages"))];
        said.extend((!steps.is_empty()).then(|| format!("{} {}", if steps.len() == 1 { "change" } else { "changes" }, tell::and_list(&list))));
        said.push("nami gc".into());
        println!("{}", style::out().line(&said.join(" · ")));
    }
    Ok(())
}

/// `nami storage`: reading only, as anyone (no daemon, no lock).
pub(crate) fn storage(root: &std::path::Path, json: bool) -> Result<()> {
    let store = Store::open_direct_readonly(root)?;
    profile::reading_only();
    let report = crate::storage::report(&store)?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        crate::storage::print(&report);
    }
    Ok(())
}

/// `nami diff` of a profile's changes.
pub(crate) fn diff(profiles: &Profiles, from: Option<u64>, to: Option<u64>, raw: bool, json: bool) -> Result<()> {
    let gens = profiles.generations()?;
    let to = match to.or(profiles.current_generation()) {
        Some(g) => g,
        None => anyhow::bail!("nothing installed yet"),
    };
    let from = match from {
        Some(g) => g,
        None => *gens.iter().filter(|&&g| g < to).last().context("no earlier change to compare with")?,
    };
    // The nami file each was applied from: going back to a step puts it back as it was.
    let (a, b) = (profiles.manifest(from)?.config, profiles.manifest(to)?.config);
    let label = |g: u64| if Some(g) == profiles.current_generation() { "now".to_string() } else { format!("change {g}") };
    let files: Vec<file_diff::FileDiff> = match (&a, &b) {
        (Some(a), Some(b)) => {
            let name = std::path::Path::new(&b.file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| b.file.clone());
            vec![file_diff::file(&name, &a.text, &b.text, &label(from), &label(to))]
        }
        _ => Vec::new(),
    };
    if json {
        println!("{}", serde_json::json!({ "from": from, "to": to, "files": files }));
    } else if raw {
        files.iter().for_each(|f| print!("{}", f.raw));
    } else {
        let paint = style::out();
        let when = tell::ago(profiles.manifest(to)?.created, tell::now());
        println!("{}", paint.line(&format!("change {from} → {} ({when})", label(to))));
        let d = profiles.diff(from, to)?;
        // What the file says of its apps, the apps' lines say already (with versions).
        let said_apps = !d.added.is_empty() || !d.removed.is_empty() || !d.changed.is_empty();
        let files: Vec<file_diff::FileDiff> = files.into_iter().map(|mut f| {
            if said_apps {
                f.sections.retain(|s| s.key != ["packages"]);
            }
            f
        }).collect();
        if d.is_empty() && files.iter().all(|f| f.sections.is_empty()) {
            println!("{}", paint.line("✓ No changes"));
        }
        d.print();
        let mut out = String::new();
        files.iter().filter(|f| !f.sections.is_empty() || f.unreadable.is_some()).for_each(|f| file_diff::print(f, &mut out));
        if !out.is_empty() {
            println!();
        }
        print!("{}", paint.lines(&out, style::Paint::diff_line));
    }
    Ok(())
}

/// `nami undo` in a profile.
pub(crate) fn rollback(profiles: &Profiles, generation: Option<u64>) -> Result<()> {
    let (current, target) = ops::rollback(&profiles, generation)?;
    let paint = style::out();
    profiles.diff(current, target)?.print();
    let mut said = format!("✓ Back at change {target}");
    let whose = if profiles.is_system() { "system.nami" } else { "your apps" };
    let mut named = false;
    // The config file is the source of truth: bring it back to match.
    if let Some(snap) = profiles.manifest(target)?.config {
        let file = PathBuf::from(&snap.file);
        if std::fs::read_to_string(&file).ok().as_deref() != Some(snap.text.as_str()) {
            std::fs::write(&file, &snap.text)?;
            if let Some(lock_text) = &snap.lock {
                std::fs::write(lock::path_for(&file), lock_text)?;
            }
            said += &format!(" · {} and its nami.lock are as they were then", crate::cmd::apply::applied_file(&file, profiles));
            named = true;
        }
    } else if let Some(snap) = profiles.manifest(current)?.config {
        // That generation was made before this file was in use (by
        // `nami install`, say): bring the file's packages in line with
        // it, so the next apply doesn't undo the rollback.
        let file = PathBuf::from(&snap.file);
        if let Ok(text) = std::fs::read_to_string(&file) {
            let have: Vec<String> = profiles.manifest(target)?.packages.into_iter().map(|e| e.attr).collect();
            let listed: Config = config::parse_config(&text, &snap.file).unwrap_or_default();
            let mut out = text.clone();
            for p in listed.packages.iter().filter(|p| !have.contains(p)) {
                for op in config::remove_package(&out, p) {
                    out = config_edit::apply(&out, &op)?;
                }
            }
            for p in have.iter().filter(|p| !listed.packages.contains(p)) {
                for op in config::add_package(&out, p)? {
                    out = config_edit::apply(&out, &op)?;
                }
            }
            if out != text {
                std::fs::write(&file, &out)?;
                said += &format!(" · {} now lists its apps", crate::cmd::apply::applied_file(&file, profiles));
                named = true;
            }
        }
    }
    if !named {
        said += &format!(" · {whose}");
    }
    println!("{}", paint.line(&said));
    let undo = crate::tell::undo_command(profiles.is_system());
    println!("{}", paint.line(&format!("  Go forward again with {undo} {current}")));
    Ok(())
}

/// `nami gc`.
pub(crate) fn gc(store: &Store, profiles: &Profiles, old: bool, keep: Option<usize>, older_than: Option<String>, now: bool) -> Result<()> {
    let policy = profile::Prune {
        all: old,
        keep: if now { Some(cleanup_keep(&profiles)) } else { keep },
        newer_than: older_than.as_deref().map(profile::parse_duration).transpose()?,
    };
    let mut removed = profiles.prune(policy)?;
    // On namiOS the system's changes are its systems (the one running, the one
    // before it and what starts next stay, for the boot menu).
    if profiles.is_system() && crate::nami_os::here() {
        let os = crate::nami_os::require()?;
        removed = os.remove(store.root(), &os.pick(store.root(), &policy));
    }
    let paint = style::out();
    let changes: Vec<String> = removed.iter().map(u64::to_string).collect();
    let went = (!removed.is_empty()).then(|| format!("{} {} removed", if removed.len() == 1 { "change" } else { "changes" }, tell::and_list(&changes)));
    if profiles.is_system() {
        let (paths, bytes) = store.gc(&profile::all_roots(&store)?)?;
        let mut said = if paths == 0 { vec!["✓ Nothing to free · everything kept is in use".to_string()] } else { vec![format!("✓ Freed {}", tell::size(bytes)), format!("{} no longer used", tell::count(paths, "package", "packages"))] };
        said.extend(went);
        println!("{}", paint.line(&said.join(" · ")));
    } else {
        let mut said = vec![went.map_or("✓ No old changes to remove".to_string(), |w| format!("✓ {}{}", w[..1].to_uppercase(), &w[1..]))];
        said.push("an administrator frees the space: sudo nami gc".into());
        println!("{}", paint.line(&said.join(" · ")));
    }
    Ok(())
}

