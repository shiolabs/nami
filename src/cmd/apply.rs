//! A nami file made real: `nami apply`, `check`, `show`, `options`, `layers`, `status`
//! and `export`; and what an apply says when it's done.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::build::Builder;
use crate::config::Config;
use crate::index::Index;
use crate::profile::{Entry, Profiles};
use crate::store::Store;
use crate::cmd::files::{CHILD_APPS, base_of, default_config, ensure_system_config, is_system_config, personal_config, system_config};
use crate::cmd::generations::clean_up_early;
use crate::cmd::setup::{repo_note_seen, repo_note_set_seen, repo_system_file, sha256_hex, system_use};
use crate::{auto_update, bundle_app, casks, cleanup, config, config_edit, errors, etc, flathub, foreign, hardware, home, import, index, lock, log, machine, oldnames, ops, profile, project, python_env, replace, schema, services, store, style, tell};

/// What `nami check` checks when it isn't told: the nearest project's
/// file (as it's reached from `cwd`, `../project.nami`), or else the
/// computer's (`default_config`).
pub(crate) fn check_default(cwd: &std::path::Path) -> Option<PathBuf> {
    project::find(cwd).map(|f| config::path_from(cwd, &f))
}

/// `nami check` of one file (`-`: stdin).
fn check_file(file: &std::path::Path, base: Option<PathBuf>, index: Option<&Index>) -> Result<Vec<config::Diagnostic>> {
    let text = if file.as_os_str() == "-" {
        let mut t = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)?;
        t
    } else {
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?
    };
    Ok(check_text(file, &text, base, index))
}

/// `nami check` of `text`, as the file `file` (`-`: stdin).
pub(crate) fn check_text(file: &std::path::Path, text: &str, base: Option<PathBuf>, index: Option<&Index>) -> Vec<config::Diagnostic> {
    let text = text.to_string();
    let base = base.unwrap_or_else(|| file.parent().filter(|p| !p.as_os_str().is_empty() && file.as_os_str() != "-").unwrap_or(std::path::Path::new(".")).to_path_buf());
    let system = is_system_config(file) || file.file_name().is_some_and(|n| oldnames::is_named(n, config::SYSTEM_FILE));
    let mut diags = if is_system_config(file) {
        config::check_tier(&text, &base, index, config::Tier::System)
    } else if file == personal_config() {
        config::check_tier(&text, &base, index, config::Tier::User)
    } else if project::is_project(file) {
        config::check_project(&text, &base, index)
    } else {
        config::check(&text, &file.display().to_string(), &base, index)
    };
    // Whether each secret opens here (asking for nothing).
    diags.extend(config::secret_notes(&text, system));
    // On another distribution: what only namiOS acts on (a config repo's
    // folder for another computer is that one's, said nothing about here).
    let another = config::in_config_repo(&base) && std::fs::canonicalize(&base).ok().and_then(|b| b.file_name().map(|f| f.to_string_lossy().into_owned())) != config::hostname();
    if system && !crate::nami_os::here() && (is_system_config(file) || !another) {
        diags.extend(config::not_on_this_distro(&text, crate::machine::distro_name().as_deref(), index));
    }
    // A package for hardware this computer hasn't.
    if is_system_config(file) {
        let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        diags.extend(hardware::unmatched(&hardware::sys_root(), &file).into_iter().map(|m| config::Diagnostic { line: 0, column: 0, severity: config::Severity::Note, message: m, hint: None }));
    }
    diags.sort_by_key(|d| d.line);
    diags
}

/// The file `nami layers` is about: the one named (a project's folder:
/// its project.nami), else the project you're in, else the system file
/// for admins and your user file for others.
pub(crate) fn layers_file(file: Option<PathBuf>) -> PathBuf {
    match file {
        Some(f) if f.is_dir() => project::file_in(&f).unwrap_or(f),
        Some(f) => f,
        None => std::env::current_dir().ok().and_then(|d| project::find(&d)).unwrap_or_else(default_config),
    }
}

/// `nami layers`: what the file includes from repos, at which commits.
pub(crate) fn layers_command(file: Option<PathBuf>, update: bool, json: bool) -> Result<()> {
    let file = layers_file(file);
    let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let used = if project::is_project(&file) {
        project::read_with_layers(&file)?.1
    } else {
        config::load_with_layers(&text, &file.display().to_string(), &base_of(&file))?.1
    };
    if json {
        let mut seen = std::collections::BTreeSet::new();
        let layers: Vec<serde_json::Value> = used.iter().filter(|u| seen.insert(u.include.clone())).map(|u| serde_json::json!({ "include": u.include, "rev": u.pin.rev, "url": u.pin.url })).collect();
        println!("{}", serde_json::json!({ "file": file, "lock": lock::path_for(&file), "layers": layers }));
        return Ok(());
    }
    if used.is_empty() {
        println!("{} includes no layers from repos", file.display());
        return Ok(());
    }
    let mut seen = std::collections::BTreeSet::new();
    for u in used.iter().filter(|u| seen.insert(u.include.clone())) {
        println!("{}  {}  {}", u.include, &u.pin.rev[..12], u.pin.url);
    }
    println!("pinned in {}{}", lock::path_for(&file).display(), if update { "" } else { "; `nami layers --update` moves them to the newest commits" });
    Ok(())
}

/// Make the profile match a nami file, keeping nami.lock's versions.
pub(crate) fn apply_config(store: &Store, profiles: &Profiles, file: &std::path::Path, locked: bool, upgrade: bool) -> Result<()> {
    apply_config_with(store, profiles, file, locked, upgrade, false)
}

/// The profile's services with their state, for `nami status`.
fn services_status(profiles: &Profiles) -> Result<Vec<serde_json::Value>> {
    let Some(s) = profiles.current()?.and_then(|m| m.services) else { return Ok(Vec::new()) };
    let Some(target) = profiles.services_target() else { return Ok(Vec::new()) };
    let names: Vec<String> = s.units.iter().map(|u| u.name.clone()).collect();
    let states = services::states(&target, &names);
    Ok(s.units.iter().zip(states).map(|(u, state)| serde_json::json!({ "name": u.service, "unit": u.name, "user": s.user, "enable": u.enable, "state": state })).collect())
}

/// The daily update's apply of a file that's as last applied: with `everything`, all of it at
/// the newest (as `nami upgrade`), else only its apps from Homebrew's casks (a person's, on a
/// Mac); either way an app that's open now stays as it is, for the next day's.
pub(crate) fn apply_config_daily(store: &Store, profiles: &Profiles, file: &std::path::Path, everything: bool) -> Result<()> {
    apply_config_as(store, profiles, file, false, everything, casks::Upgrade::NewestClosed, false)
}

/// `apply_config`; with `dry_run`, only say what would change.
pub(crate) fn apply_config_with(store: &Store, profiles: &Profiles, file: &std::path::Path, locked: bool, upgrade: bool, dry_run: bool) -> Result<()> {
    apply_config_as(store, profiles, file, locked, upgrade, if upgrade { casks::Upgrade::Newest } else { casks::Upgrade::No }, dry_run)
}

/// `apply_config_with`, with how far Homebrew's casks move said apart (`cask_upgrade`).
fn apply_config_as(store: &Store, profiles: &Profiles, file: &std::path::Path, locked: bool, upgrade: bool, cask_upgrade: casks::Upgrade, dry_run: bool) -> Result<()> {
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    anyhow::ensure!(
        !project::has_project_keys(&text) && file.file_name().is_none_or(|n| n != project::FILE),
        "{} is a project's file (its tools are for `nami shell` and `nami run` there); `nami apply` is for system.nami and user.nami",
        file.display()
    );
    let base = base_of(file);
    // ~/.config/nami may be a config repo's checkout (or become one): this
    // computer's own files there (secrets key, shell setup) stay out of it.
    if !dry_run && file == personal_config() {
        if let Some(dir) = lock::path_for(file).parent().filter(|d| d.is_dir()) {
            machine::ignore_own_files(dir);
        }
    }
    let index = Index::load(&store).ok();
    // Lint first: better to say what's wrong than to fail halfway.
    let tier = if profiles.is_system() { config::Tier::System } else { config::Tier::User };
    let lock_path = lock::path_for(file);
    let old_lock = lock::load(&lock_path)?;
    // A name the catalog doesn't have (renamed or dropped by nixpkgs since
    // the file was written) is left out, loudly, rather than holding up the
    // rest: a setup from a years-old repo still comes up. What nami.lock
    // pins is installed as pinned.
    let mut left_out: Vec<(String, String)> = Vec::new();
    let mut said = Vec::new();
    let problems: Vec<_> = config::check_tier(&text, &base, if locked { None } else { index.as_ref() }, tier)
        .into_iter()
        .filter(|d| d.severity == config::Severity::Error)
        .filter(|d| match d.message.strip_suffix(" isn't in the catalog").or_else(|| d.message.strip_suffix(casks::NOT_A_CASK)) {
            Some(p) if old_lock.packages.contains_key(p) => false,
            Some(p) => {
                left_out.push((p.to_string(), d.hint.clone().unwrap_or_default()));
                // As `nami check` says it, and that it's left out.
                said.push(config::Diagnostic { severity: config::Severity::Warning, message: format!("{}, so it's left out", d.message), hint: d.hint.clone(), ..*d });
                false
            }
            None => true,
        })
        .collect();
    for d in &said {
        eprintln!("{}", d.shown(&file.display().to_string(), style::err()));
    }
    if !problems.is_empty() {
        for d in &problems {
            eprintln!("{}", d.shown(&file.display().to_string(), style::err()));
        }
        return Err(errors::coded_hint(errors::Code::InvalidFile, format!("{} has {} problem(s); nothing was changed", file.display(), problems.len()), format!("`nami check {} --json` lists them, by line.", file.display())));
    }
    let config: Config = config::load_text(&text, &file.display().to_string(), &base)?;
    // A Mac: packages, caches, home files and env; what's Linux's own is left out, said.
    let config = if index::MAC { mac_only(config, file) } else { config };
    // Homebrew's casks: a Mac's apps (casks.rs), installed below; Linux leaves them out.
    let (config, cask_wanted) = casks_of(config, index.as_ref(), file, &left_out);
    // What a removal's last line says of the space the change before still holds.
    tell::set_cleanup(config.cleanup.unwrap_or_default().words(), profiles.is_system() && profile::is_root());
    // The disk's nearly full: old changes go now (by `cleanup:`, whatever their age).
    if !dry_run && cleanup::space_low(&store.phys(store::STORE_DIR)) {
        clean_up_early(store, profiles, &config);
    }
    // Updates by themselves, or not (`updates:`), on another distribution: its timer.
    if tier == config::Tier::System && !dry_run && profile::is_root() && !crate::nami_os::here() && is_system_config(file) {
        if let Err(e) = auto_update::set_up(store.root(), config::updates_itself(&text)) {
            eprintln!("{}: updates: the daily update's timer stays as it was ({e:#})", file.display());
        }
    }
    if tier == config::Tier::User && profile::is_child() && (!config.packages.is_empty() || !config.services.is_empty()) {
        anyhow::bail!("{}: {CHILD_APPS}; take packages: and services: out of it", file.display());
    }
    if config.os.is_some() && tier == config::Tier::System {
        // A computer nami usb made: the file is its whole system.
        if crate::nami_os::here() {
            anyhow::ensure!(profile::is_root(), "changing the system takes root (an admin's `nami apply` goes through the daemon)");
            return crate::nami_os::require()?.switch(store, file, &config, dry_run);
        }
        if let Some(keys) = config::only_on_nami_os(&text) {
            let work = if keys.contains(" and ") { "work" } else { "works" };
            let distro = crate::machine::distro_name().unwrap_or_else(|| "This distribution".into());
            eprintln!("{}: {keys} only {work} on namiOS. {distro} keeps its own (`nami check` says each line)", file.display());
        }
    }
    // What its sets list is checked here (the file's own was above). A set
    // of nami's (namios) is a whole computer: on one nami didn't make, its
    // own system stays as it is.
    if let (Some(index), false) = (index.as_ref(), locked) {
        for p in &config.packages {
            let plain = config::is_plain_entry(p);
            if plain && config::is_set(p, index) && !left_out.iter().any(|(l, _)| l == p) {
                eprintln!("{}: leaving out {p}: it's a set for namiOS; {} keeps its own system", file.display(), crate::machine::distro_name().unwrap_or_else(|| "this distribution".into()));
                left_out.push((p.clone(), String::new()));
            } else if plain && index.get(p).is_err() && !old_lock.packages.contains_key(p) && !left_out.iter().any(|(l, _)| l == p) {
                eprintln!("{}: leaving out {p}: it isn't in the catalog (a set lists it)", file.display());
                left_out.push((p.clone(), String::new()));
            } else if plain && !left_out.iter().any(|(l, _)| l == p) {
                // A program nami sets up on its own computers: here it comes alone.
                let parts = config::package_setup(p);
                if !parts.is_empty() {
                    let name = index::split_version(p).0;
                    let own = match import::deb_name(name) {
                        Some(deb) if std::path::Path::new("/usr/bin/apt").exists() => format!("for those, this system's own: sudo apt install {deb}"),
                        _ => "for those, this system's own package".to_string(),
                    };
                    let parts = match parts.split_last() {
                        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
                        _ => parts.join(""),
                    };
                    eprintln!("{}: {p}: the program alone here; on a computer nami makes (`nami usb`) it also gets {parts}; {own}", file.display());
                }
            }
        }
    }
    anyhow::ensure!(
        !is_system_config(file) || (config.home.is_none() && config.settings.is_none()),
        "{}: home settings are each person's own; put them in your ~/user.nami (this is system.nami)",
        file.display()
    );
    let mut service_list = services::parse_all(&config.services)?;
    let mut services_skipped = Vec::new();
    if !service_list.is_empty() {
        if let services::Backend::Unsupported(why) = services::backend(store.root()) {
            // Everything else still comes up; the services wait for a systemd.
            services_skipped = std::mem::take(&mut service_list).into_iter().map(|s| s.name).collect();
            eprintln!("{}: leaving out the services ({}): {why}", file.display(), services_skipped.join(", "));
        }
    }
    anyhow::ensure!(config.files.is_empty() || profiles.is_system(), "{}: files: writes /etc, so it's only for system.nami", file.display());
    // Caches first: recipes below may be downloadable from them.
    if profiles.is_system() {
        store.set_caches(&config.caches)?;
    } else {
        store.check_user_caches(&config.caches)?;
    }

    let need_index = || index.as_ref().ok_or_else(|| anyhow::anyhow!("no package catalog here yet; run `sudo nami update --from ...` (or use --locked with a complete nami.lock)"));
    let mut wanted = Vec::new();
    let mut locked_now = std::collections::BTreeMap::new();
    let builder_for = |index| {
        let mut b = Builder::new(&store, index);
        b.allow_unfree = config.allow_unfree();
        b
    };
    // Flathub's apps: each person's own (flatpak --user), installed below.
    let from_flathub: Vec<String> = config.packages.iter().filter_map(|p| flathub::id(p)).map(String::from).collect();
    anyhow::ensure!(
        from_flathub.is_empty() || !profiles.is_system(),
        "{}: Flathub's apps are each person's own (flatpak --user): list them in your ~/user.nami",
        file.display()
    );
    // Bundles (`bundle:github:ada/myapp`): each person's own too, installed below.
    let bundles: Vec<String> = config.packages.iter().filter(|p| bundle_app::is_entry(p)).cloned().collect();
    anyhow::ensure!(bundles.is_empty() || !profiles.is_system(), "{}: bundles are each person's own apps: list them in your ~/user.nami", file.display());
    // A package service's package comes with it.
    let mut names: Vec<String> = config.packages.iter().filter(|p| !left_out.iter().any(|(l, _)| l == *p) && !flathub::is_flathub(p) && !bundle_app::is_entry(p)).cloned().collect();
    names.extend(services::packages(&service_list).into_iter().filter(|p| !config.packages.contains(p)));
    for name in &names {
        if ops::is_remote(name) {
            let index = need_index()?;
            let b = builder_for(index);
            let pinned = old_lock.packages.get(name).filter(|_| !upgrade);
            let (entry, rev) = match pinned {
                Some(l) if l.paths.iter().all(|p| store.is_valid(store::base_name(p))) => (Entry { attr: name.clone(), name: l.name.clone(), paths: l.paths.clone() }, l.rev.clone()),
                Some(l) => {
                    let (e, sha) = ops::resolve_remote(&b, &store, name, l.rev.as_deref())?;
                    anyhow::ensure!(!locked || e.paths == l.paths, "{name} builds something different from what the lock says");
                    (e, Some(sha))
                }
                None => {
                    anyhow::ensure!(!locked, "{name} isn't in the lock; run without --locked to add it");
                    let (e, sha) = ops::resolve_remote(&b, &store, name, None)?;
                    (e, Some(sha))
                }
            };
            locked_now.insert(name.clone(), lock::Locked { name: entry.name.clone(), paths: entry.paths.clone(), rev, inputs: Vec::new() });
            wanted.push(entry);
            continue;
        }
        let entry = if Builder::is_recipe(name) {
            let index = need_index()?;
            let e = ops::resolve(&builder_for(index), index, &base, name)?;
            if locked && old_lock.packages.get(name).map(|l| &l.paths) != Some(&e.paths) {
                anyhow::bail!("{name} builds something different from what nami.lock says (the recipe or its inputs changed); apply without --locked to update the lock");
            }
            e
        } else if let (false, Some(l), true) = (upgrade, old_lock.packages.get(name), python_env::parse(name).is_some()) {
            // A Python with packages: made here again from what the lock says it's made of.
            python_env::pinned(&store, name, l, locked)?
        } else if let (false, Some(l)) = (upgrade, old_lock.packages.get(name)) {
            if let Some(index) = index.as_ref().filter(|i| i.get(name).is_ok_and(|p| p.install_paths() == l.paths)) {
                builder_for(index).ensure_catalog(name)?; // unfree policy; build what the cache lacks
            }
            Entry { attr: name.clone(), name: l.name.clone(), paths: l.paths.clone() }
        } else {
            anyhow::ensure!(!locked, "{name} isn't in nami.lock; apply without --locked to add it");
            let index = need_index()?;
            ops::resolve(&builder_for(index), index, &base, name)?
        };
        let inputs = python_env::inputs(&store, &entry);
        locked_now.insert(name.clone(), lock::Locked { name: entry.name.clone(), paths: entry.paths.clone(), rev: None, inputs });
        wanted.push(entry);
    }
    // What can be neither downloaded nor built here (nixpkgs' own build of
    // it failed, so no cache has it) is left out too, not the whole setup.
    let mut unavailable = Vec::new();
    if let Some(index) = index.as_ref() {
        // (A cache that can't be asked, or answers badly signed, stops here: that isn't "no
        // cache has it", and leaving the package out would hide it.)
        let mut keep = Vec::with_capacity(wanted.len());
        for e in &wanted {
            let from_catalog = !Builder::is_recipe(&e.attr) && !ops::is_remote(&e.attr);
            let buildable = index.get(&e.attr).is_ok_and(|p| p.closure.is_some());
            let mut ok = !from_catalog || buildable;
            if !ok {
                ok = true;
                for p in &e.paths {
                    if !store.is_valid(store::base_name(p)) && !store.in_a_cache(store::base_name(p)).with_context(|| format!("checking {} can be downloaded", e.attr))? {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                eprintln!("{}: leaving out {}: no binary cache has {} (nixpkgs couldn't build it), and the catalog has no way to build it here", file.display(), e.attr, e.name);
                unavailable.push(e.attr.clone());
            }
            keep.push(ok);
        }
        let mut keep = keep.into_iter();
        wanted.retain(|_| keep.next().unwrap_or(true));
    }
    // A library swapped in what uses it (replace:); the lock keeps what was asked for.
    let plan = config.replace_plan();
    if !plan.is_empty() {
        let index = need_index()?;
        let packages = wanted.iter_mut().map(|e| (e.attr.as_str(), &mut e.paths)).collect();
        replace::apply(&builder_for(index), index, &base, &plan, packages)?;
    }
    let home = if config.home.is_some() || config.settings.is_some() {
        let empty = home::Home::default();
        Some(home::build(store, config.home.as_ref().unwrap_or(&empty), &base, config.settings.clone().unwrap_or_default())?)
    } else {
        None
    };
    let source = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf()).display().to_string();
    let units = if service_list.is_empty() {
        None
    } else {
        // Units are read out of the packages, so they must be here first.
        store.realise(&wanted.iter().flat_map(|e| e.paths.clone()).collect::<Vec<_>>())?;
        Some(services::build(store, &service_list, &wanted, !profiles.is_system(), &source)?)
    };
    let etc_files = if config.files.is_empty() { None } else { Some(etc::build(store, &config.files, &base)?) };
    if dry_run {
        wanted.extend(flathub::entries(&from_flathub));
        wanted.extend(bundle_app::entries(&foreign::Places::here(), &bundles));
        wanted.extend(casks::entries(&cask_wanted, &old_lock));
        return dry_run_report(store, profiles, file, &wanted, units.as_ref(), etc_files.as_ref());
    }
    // What's missing comes from Flathub now; what can't is left out, loudly, as above.
    let (from_flathub, flathub_left) = flathub::install(&from_flathub);
    wanted.extend(from_flathub);
    for (p, why) in &flathub_left {
        eprintln!("{}: leaving out {p}: {why}", file.display());
    }
    // Homebrew's casks: the lock's as they are (the store keeps them), the rest downloaded
    // from their developers and checked now (casks.rs); what can't be is left out, loudly.
    let (from_casks, casks_left) = if cask_wanted.is_empty() { (Vec::new(), Vec::new()) } else { casks::install(store, &cask_wanted, &old_lock, cask_upgrade, locked, profiles.is_system()) };
    for c in from_casks {
        locked_now.insert(c.entry.attr.clone(), c.lock);
        wanted.push(c.entry);
    }
    // `nami install` of a cask that can't be had: said as an error, and nothing changes (the
    // file's put back as it was). An apply goes on without it.
    if let Some((p, why)) = casks_left.iter().find(|(p, _)| tell::installing().contains(p)) {
        anyhow::bail!("{p} wasn't installed: {why}");
    }
    for (p, why) in &casks_left {
        eprintln!("{}: leaving out {p}: {why}", file.display());
    }
    // The bundles listed: what's missing comes from where each updates, checked
    // against its key. Steps of the history (src/bundle_app.rs): one no longer
    // listed comes off as the profile switches (bundle_app::follow).
    if !bundles.is_empty() && !profile::is_root() {
        let (from_bundles, bundles_left) = bundle_app::install_listed(&foreign::Places::here(), &bundles);
        wanted.extend(from_bundles);
        for (p, why) in &bundles_left {
            eprintln!("{}: leaving out {p}: {why}", file.display());
        }
    }
    let runtime_lock = (!upgrade).then(|| old_lock.runtime.clone());
    let before = profiles.current_generation();
    let extras = profile::Extras { services: units, etc: etc_files };
    let after = profiles.commit_with(index.as_ref(), wanted, Some(config.caches), Some(home), Some(extras), runtime_lock)?;
    report(profiles, before, after)?;

    let now = lock::load(&lock_path)?;
    let new_lock = lock::Lock {
        version: 1,
        catalog: if upgrade || old_lock.catalog.is_none() { lock::catalog(&store.var_dir()) } else { old_lock.catalog.clone() },
        packages: locked_now,
        runtime: profiles.current()?.map(|m| m.runtime).unwrap_or_default(),
        // What reading the file pinned (layers from repos, a recipe's
        // sources in this folder), as it is now.
        layers: now.layers,
        sources: now.sources,
    };
    if locked {
        anyhow::ensure!(new_lock.packages == old_lock.packages, "nami.lock would change; apply without --locked");
    } else if new_lock != old_lock {
        lock::save(&lock_path, &new_lock)?;
        // (Next to the file: its name says where.)
        let shown = if lock_path.parent() == file.parent() { lock::path_for(file).file_name().map_or_else(|| lock_path.display().to_string(), |n| n.to_string_lossy().into_owned()) } else { lock_path.display().to_string() };
        println!("{}", style::out().dim(&format!("Versions saved in {shown}")));
    }
    // Remember the exact config this generation came from (for install/remove
    // and rollback). Secrets are only names in it.
    let abs = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    profiles.record_config(profile::ConfigSnapshot {
        file: abs.display().to_string(),
        text: text.clone(),
        lock: std::fs::read_to_string(&lock_path).ok(),
    })?;
    // (Steps from before a person's file was kept beside them, theirs alone:
    // moved there now. The computer's are, as it updates or starts.)
    if !profiles.is_system() {
        if let Err(e) = profiles.seal_snapshots() {
            eprintln!("your earlier changes still have your file in them: {e:#}");
        }
    }
    if let Some(keep) = config.history {
        let removed = profiles.prune(profile::Prune { keep: Some(keep.max(1)), ..Default::default() })?;
        if !removed.is_empty() {
            let list: Vec<String> = removed.iter().map(u64::to_string).collect();
            println!("{}", style::out().dim(&format!("Keeping the last {keep} changes (history: {keep}): {} went", tell::and_list(&list))));
        }
    }
    if !left_out.is_empty() {
        let names: Vec<&str> = left_out.iter().map(|(p, _)| p.as_str()).collect();
        eprintln!("everything else is set up; left out (not in the catalog): {}. Fix their names in {}", names.join(", "), file.display());
    }
    if !unavailable.is_empty() {
        eprintln!("everything else is set up; left out (can't be downloaded or built right now): {}. A later catalog may have them: `nami update --refresh`, then `nami apply` again", unavailable.join(", "));
    }
    if !flathub_left.is_empty() {
        let names: Vec<&str> = flathub_left.iter().map(|(p, _)| p.as_str()).collect();
        eprintln!("everything else is set up; left out (Flathub's, not installed): {}. `nami apply` again tries again", names.join(", "));
    }
    if !casks_left.is_empty() {
        let names: Vec<&str> = casks_left.iter().map(|(p, _)| p.as_str()).collect();
        eprintln!("everything else is set up; left out (Homebrew's casks, not installed): {}. `nami apply` again tries again", names.join(", "));
    }
    if !services_skipped.is_empty() {
        eprintln!("everything else is set up; the services ({}) need systemd, which isn't running here", services_skipped.join(", "));
    }
    finish(file, profiles);
    Ok(())
}

/// What `apply` would do, for `--dry-run`.
fn dry_run_report(store: &Store, profiles: &Profiles, file: &std::path::Path, wanted: &[Entry], units: Option<&services::ServicesState>, etc_files: Option<&etc::EtcState>) -> Result<()> {
    let have = profiles.current_packages()?;
    let mut lines = Vec::new();
    for e in &have {
        match wanted.iter().find(|w| w.attr == e.attr) {
            None => lines.push(format!("- {}", e.name)),
            Some(w) if w.paths != e.paths => lines.push(format!("~ {}: {} -> {}", e.attr, e.name, w.name)),
            _ => {}
        }
    }
    lines.extend(wanted.iter().filter(|w| !have.iter().any(|e| e.attr == w.attr)).map(|w| format!("+ {}", w.name)));
    let apps = lines.len();
    if let Some(target) = profiles.services_target() {
        let etc_changed = if profiles.is_system() { etc::changes(store.root(), &store.var_dir(), etc_files) } else { Vec::new() };
        let (dir, live) = (target.logical.clone(), target.live);
        let activation = services::Activation::new(target, std::path::Path::new(""), units, &etc_changed);
        lines.extend(activation.overrides());
        let plan = &activation.plan;
        lines.extend(plan.describe().into_iter().map(|l| match l.split_once(' ') {
            Some((verb @ ("write" | "delete"), f)) => format!("{verb} {dir}/{f}"),
            _ => l,
        }));
        if !plan.is_empty() && !live {
            lines.push("(services won't start or stop here: no running systemd)".into());
        }
    }
    let mut files = Vec::new();
    if profiles.is_system() {
        files = etc::describe(store.root(), &store.var_dir(), etc_files);
    }
    if errors::json() {
        // (What the lines say, apart: the packages, the services' and /etc's.)
        let (mut add, mut remove, mut change, mut services) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for l in &lines {
            if let Some(n) = l.strip_prefix("+ ") {
                add.push(serde_json::json!(n));
            } else if let Some(n) = l.strip_prefix("- ") {
                remove.push(serde_json::json!(n));
            } else if let Some((attr, rest)) = l.strip_prefix("~ ").and_then(|r| r.split_once(": ")) {
                let (from, to) = rest.split_once(" -> ").unwrap_or((rest, ""));
                change.push(serde_json::json!({ "attr": attr, "from": from, "to": to }));
            } else {
                services.push(serde_json::json!(l));
            }
        }
        let nothing = lines.is_empty() && files.is_empty();
        println!("{}", serde_json::json!({ "dry_run": true, "changes": !nothing, "packages": { "add": add, "remove": remove, "change": change }, "services": services, "files": files }));
        return Ok(());
    }
    let paint = style::out();
    let name = file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into_owned());
    if lines.is_empty() && files.is_empty() {
        println!("{}", paint.line(&format!("✓ Nothing to change · {name} is in effect")));
        return Ok(());
    }
    // By kind, as a change says it: the apps (`+ firefox 160.0`), then what
    // the services and /etc's files would do.
    let app_lines: Vec<String> = lines[..apps].iter().map(|l| match (l.strip_prefix("+ "), l.strip_prefix("- "), l.strip_prefix("~ ").and_then(|r| r.split_once(": "))) {
        (Some(n), _, _) => format!("+ {}", tell::shown(n)),
        (_, Some(n), _) => format!("− {}", tell::shown(n)),
        (_, _, Some((attr, rest))) => {
            let (a, b) = rest.split_once(" -> ").unwrap_or((rest, ""));
            let ((_, a), (_, b)) = (tell::name_version(a), tell::name_version(b));
            if a.is_empty() || b.is_empty() || a == b { format!("~ {attr}") } else { format!("~ {attr} {a} → {b}") }
        }
        _ => l.clone(),
    }).collect();
    let (services, notes): (Vec<String>, Vec<String>) = lines[apps..].iter().cloned().partition(|l| !l.starts_with('('));
    let n = lines.len() + files.len();
    for l in tell::grouped(&[("Apps", app_lines), ("Services", services), ("Files", files)]) {
        println!("{}", paint.line(&l));
    }
    for n in notes {
        println!("{}", paint.dim(&n));
    }
    let again = if profiles.is_system() { "sudo nami apply" } else { "nami apply" };
    println!("{}", paint.line(&format!("Dry run · {} · nothing was changed yet · {again}", tell::count(n, "change", "changes"))));
    Ok(())
}

/// When the profile is managed with a nami file, change that file (keeping
/// its comments) and apply it, so the file stays the one source of truth.
/// If the apply fails, the file is put back.
pub(crate) fn edit_and_apply(store: &Store, profiles: &Profiles, file: &std::path::Path, ops: &[config_edit::Op], upgrade: bool) -> Result<()> {
    // system.nami from a config repo: the repo's copy changes too.
    let repo = repo_system_file().filter(|_| is_system_config(file));
    let mut files = vec![file.to_path_buf()];
    files.extend(repo.clone());
    let mut changed = Vec::new();
    for f in &files {
        let original = std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let mut text = original.clone();
        for op in ops {
            text = config_edit::apply(&text, op).with_context(|| format!("editing {}", f.display()))?;
        }
        if text != original {
            // (tray's computer.tray, outside a repo: it becomes system.nami as it changes.)
            let f = &oldnames::for_writing(f);
            std::fs::write(f, &text)?;
            // (Said by the last line when it's an install or a removal: `· in user.nami`.)
            if !tell::has_intent() {
                println!("{}", style::out().line(&format!("updated {}", f.display())));
            }
            changed.push((f.clone(), original));
        }
    }
    if let Err(e) = apply_config(store, profiles, file, false, upgrade) {
        for (f, original) in &changed {
            std::fs::write(f, original)?;
            log::settle();
            eprintln!("{}", style::err().dim(&format!("{} is as it was before", f.display())));
        }
        return Err(e);
    }
    // Both changed alike: as they are now is what they were last the same as.
    if let Some(r) = &repo {
        let (ours, theirs) = (std::fs::read_to_string(file).ok(), std::fs::read_to_string(r).ok());
        if ours.is_some() && ours == theirs {
            repo_note_set_seen(ours.as_deref().unwrap_or_default())?;
        }
    }
    if let Some(r) = repo.filter(|r| changed.iter().any(|(f, _)| f == r)) {
        let dir = r.parent().and_then(|d| d.parent()).unwrap_or(&r);
        println!("{}", style::out().line(&format!("note: {} is your config repo's: commit and push it there (git -C {} commit -am ...)", r.display(), dir.display())));
    }
    Ok(())
}

/// Print what a change did, by kind (the apps a command named are said by
/// its last line, `tell::finish`, not here too).
pub(crate) fn report(profiles: &Profiles, before: Option<u64>, after: Option<u64>) -> Result<()> {
    log::settle();
    let Some(g) = after else { return Ok(()) };
    let d = match before {
        Some(b) => profiles.diff(b, g)?,
        None => {
            let m = profiles.manifest(g)?;
            profile::Diff { added: m.packages.iter().map(|e| e.name.clone()).collect(), runtime_added: m.runtime.iter().map(|e| e.name.clone()).collect(), ..Default::default() }
        }
    };
    let named = tell::named();
    let skip: Vec<String> = d.added.iter().chain(&d.removed).chain(d.changed.iter().map(|(_, _, new)| new)).filter(|n| named.iter().any(|a| tell::same(n, a))).cloned().collect();
    d.print_but(&skip);
    let m = profiles.manifest(g)?;
    let opens = m.packages.iter().filter(|e| d.added.contains(&e.name) && skip.contains(&e.name)).map(|e| (e.attr.clone(), tell::opens(&e.attr, &e.paths, |p| profiles.store().phys(p)))).collect();
    tell::set_outcome(tell::Outcome {
        generation: Some(g),
        added: d.added.clone(),
        removed: d.removed.clone(),
        changed: d.changed.len(),
        updated: d.changed.iter().map(|(_, old, new)| (old.clone(), new.clone())).collect(),
        size_added: d.size_added,
        size_removed: d.size_removed,
        opens,
    });
    Ok(())
}

/// A nami file as a Mac takes it: its packages, caches, home files and environment. What's
/// Linux's own (services, desktop settings, Flathub's apps, bundles, files in /etc, namiOS) is
/// left out and said, so one user.nami does for both.
fn mac_only(mut config: Config, file: &std::path::Path) -> Config {
    let mut out: Vec<String> = Vec::new();
    if !config.services.is_empty() {
        out.push("services".into());
        config.services = Default::default();
    }
    if config.settings.take().is_some() {
        out.push("settings (desktop settings)".into());
    }
    if !config.files.is_empty() {
        out.push("files in /etc".into());
        config.files = Default::default();
    }
    if config.os.take().is_some() {
        out.push("namios".into());
    }
    let (linux, packages): (Vec<String>, Vec<String>) = config.packages.into_iter().partition(|p| flathub::is_flathub(p) || bundle_app::is_entry(p));
    config.packages = packages;
    out.extend(linux);
    if !out.is_empty() {
        eprintln!("{}: leaving out {}: Linux's, not on a Mac", file.display(), tell::and_list(&out));
    }
    config
}

/// The file's casks (casks.rs), taken out of its packages: on a Mac, to install them; on Linux,
/// to leave them out, said in a line (as a Mac does with Flathub's apps), so one user.nami does
/// for both. What the check left out already (no such cask) isn't one.
fn casks_of(mut config: Config, index: Option<&Index>, file: &std::path::Path, left_out: &[(String, String)]) -> (Config, Vec<casks::Wanted>) {
    if !index::MAC {
        let (macs, packages): (Vec<String>, Vec<String>) = config.packages.into_iter().partition(|p| casks::is_cask(p));
        config.packages = packages;
        if !macs.is_empty() {
            eprintln!("{}: leaving out {}: Mac apps from Homebrew's casks, not on Linux", file.display(), tell::and_list(&macs));
        }
        return (config, Vec::new());
    }
    let wanted: Vec<casks::Wanted> = casks::want(&config.packages, index, |t| casks::known(t) == Some(true)).into_iter().filter(|w| !left_out.iter().any(|(l, _)| *l == w.listed)).collect();
    config.packages.retain(|p| !wanted.iter().any(|w| w.listed == *p) && !(casks::is_cask(p) && left_out.iter().any(|(l, _)| l == p)));
    (config, wanted)
}

/// A nami file as a change's last line names it: `your user.nami`, `system.nami`.

pub(crate) fn said_file(file: &std::path::Path, profiles: &Profiles) -> String {
    let name = file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into_owned());
    if profiles.is_system() { name } else { format!("your {name}") }
}

/// A change's last lines (`✓ Installed firefox 160.0 · in user.nami …`, `✓ Applied
/// ~/user.nami …`), for `file`, made so in `profiles`.
pub(crate) fn finish(file: &std::path::Path, profiles: &Profiles) {
    log::settle();
    let paint = style::out();
    for l in tell::finish(&applied_file(file, profiles), profiles.is_system()) {
        println!("{}", paint.line(&l));
    }
}

/// A nami file as an apply names it: `system.nami`, or where it is (`~/user.nami`).
pub(crate) fn applied_file(file: &std::path::Path, profiles: &Profiles) -> String {
    if profiles.is_system() {
        return crate::config::SYSTEM_FILE.to_string();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    match file.strip_prefix(&home) {
        Ok(rel) if !home.as_os_str().is_empty() => format!("~/{}", rel.display()),
        _ => file.display().to_string(),
    }
}

/// `nami check`.
pub(crate) fn check(root: &std::path::Path, file: Option<PathBuf>, base: Option<PathBuf>, json: bool) -> Result<()> {
    let nearest = file.is_none().then(|| std::env::current_dir().ok().and_then(|d| check_default(&d))).flatten();
    if let (Some(f), false) = (&nearest, json) {
        eprintln!("{}", style::err().dim(&format!("checking {}, the nearest project's file", f.display())));
    }
    let file = file.or(nearest).unwrap_or_else(default_config);
    let index = Store::open_direct_readonly(&root).ok().and_then(|s| Index::load(&s).ok());
    // A config repo: each computer's files; a computer's folder: its
    // system.nami and each person's <login>/user.nami.
    let both = |d: &std::path::Path| -> Vec<PathBuf> { std::iter::once(oldnames::find_in(d, config::SYSTEM_FILE)).chain(config::people(d).iter().map(|p| config::person_file(d, p))).collect() };
    let files: Vec<PathBuf> = if let Some(p) = file.is_dir().then(|| project::file_in(&file)).flatten() {
        vec![p]
    } else if file.is_dir() && both(&file).iter().any(|f| f.is_file()) {
        both(&file).into_iter().filter(|f| f.is_file()).collect()
    } else if file.is_dir() {
        // Two names: a computer's folder, or shared/, holds system.nami and user.nami.
        let namis = |d: &std::path::Path| -> Vec<String> { std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| oldnames::ends_with_extension(n)).collect() };
        let loose = namis(&file);
        anyhow::ensure!(loose.is_empty(), "{}: {} at its top: what the computers share goes in {}/ as {} or {} (each lists ../{}/{})", file.display(), loose.join(", "), config::SHARED_DIR, config::SYSTEM_FILE, config::USER_FILE, config::SHARED_DIR, config::USER_FILE);
        let shared = file.join(config::SHARED_DIR);
        let odd: Vec<String> = namis(&shared).into_iter().filter(|n| !oldnames::is_named(n, config::SYSTEM_FILE) && !oldnames::is_named(n, config::USER_FILE)).collect();
        anyhow::ensure!(odd.is_empty(), "{}: {}: what's shared is {} (the computers') and {} (yours), nothing else", shared.display(), odd.join(", "), config::SYSTEM_FILE, config::USER_FILE);
        let f: Vec<PathBuf> = config::computers(&file).iter().flat_map(|c| both(&file.join(c))).filter(|f| f.is_file()).collect();
        anyhow::ensure!(!f.is_empty(), "{}: no computers there (a folder per computer, named after it: <hostname>/system.nami and each person's <hostname>/<login>/user.nami)", file.display());
        f
    } else {
        vec![file]
    };
    let mut all = serde_json::Map::new();
    let mut failed = false;
    for file in &files {
        let diags = check_file(file, base.clone(), index.as_ref())?;
        failed |= diags.iter().any(|d| d.severity == config::Severity::Error);
        if json {
            all.insert(file.display().to_string(), serde_json::to_value(&diags)?);
            continue;
        }
        let paint = style::out();
        for d in &diags {
            println!("{}", d.shown(&file.display().to_string(), paint));
        }
        if diags.iter().all(|d| d.severity == config::Severity::Note) {
            println!("{}", paint.line(&format!("✓ No problems in {}", file.display())));
        }
    }
    if json {
        // One file: its list; several: each file's.
        let out = if files.len() == 1 { all.into_iter().next().map(|(_, v)| v).unwrap_or_default() } else { serde_json::Value::Object(all) };
        println!("{out}");
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// `nami show`.
pub(crate) fn show(root: &std::path::Path, file: Option<String>, json: bool) -> Result<()> {
    let file = match file {
        Some(f) => crate::machine::locate_computer(&root, &f)?,
        None => system_config(),
    };
    let flat = config::flat(&file)?;
    if json {
        let v: serde_json::Value = serde_yaml::from_str(&flat).map_err(|e| errors::coded(errors::Code::InvalidFile, format!("{}: {e}", file.display())))?;
        println!("{v}");
    } else {
        print!("{flat}");
    }
    Ok(())
}

/// `nami options`.
pub(crate) fn options(json: bool, schema: Option<String>) -> Result<()> {
    if let Some(kind) = schema {
        let s = schema::schema(&kind).ok_or_else(|| anyhow::anyhow!("--schema {kind}: one of {}", schema::KINDS.join(", ")))?;
        println!("{}", serde_json::to_string_pretty(&s)?);
    } else if json {
        println!("{}", serde_json::to_string(config::OPTIONS)?);
    } else {
        for o in config::OPTIONS {
            println!("{}  ({})\n  {}\n\n{}\n", o.key, o.kind, o.summary, o.example.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n"));
        }
    }
    Ok(())
}

/// `nami export`.
pub(crate) fn export(profiles: &Profiles) -> Result<()> {
    match profiles.current()? {
        Some(m) => print!("{}", config::export(&m)),
        None => print!("packages:\n"),
    }
    Ok(())
}

/// `nami apply`.
pub(crate) fn apply(store: &Store, profiles: &Profiles, file: Option<PathBuf>, locked: bool, upgrade: bool, dry_run: bool, password_stdin: bool) -> Result<()> {
    if password_stdin {
        config::computer::password_from_stdin()?;
    }
    let file = match file {
        Some(f) => f,
        // system.nami from a config repo: whichever of the
        // two changed since the last apply (the repo's, or a hand
        // edit here, which goes into the repo's copy too).
        None if profiles.is_system() && profile::is_root() => match repo_system_file() {
            Some(r) => {
                let (theirs, ours) = (std::fs::read_to_string(&r).ok(), std::fs::read_to_string(system_config()).ok());
                // Both as they were when they were last the same.
                let last = repo_note_seen();
                let seen = |t: &Option<String>| last.as_deref() == Some(sha256_hex(t.as_deref().unwrap_or_default().as_bytes()).as_str());
                let (ours_changed, theirs_changed) = (last.is_some() && !seen(&ours), !seen(&theirs));
                if theirs == ours {
                    repo_note_set_seen(ours.as_deref().unwrap_or_default())?;
                    ensure_system_config(&profiles)?
                } else if ours_changed && !theirs_changed {
                    std::fs::write(&r, ours.as_deref().unwrap_or_default()).with_context(|| format!("writing {}", r.display()))?;
                    repo_note_set_seen(ours.as_deref().unwrap_or_default())?;
                    eprintln!("{}", style::err().line(&format!("note: {} changed here, and your config repo's copy with it ({}): commit and push it there", system_config().display(), r.display())));
                    ensure_system_config(&profiles)?
                } else if ours_changed {
                    anyhow::bail!("{} and {} (your config repo's) both changed since the last apply: make them the same, then apply", system_config().display(), r.display());
                } else {
                    eprintln!("system.nami: {} (your config repo's)", r.display());
                    system_use(r.parent().unwrap())?
                }
            }
            None => ensure_system_config(&profiles)?,
        },
        // Yours, without sudo: the system's is `sudo nami apply` (or --system, by the daemon).
        None => personal_config(),
    };
    // system.nami applies to the computer; any other to you.
    let mine;
    let target = if is_system_config(&file) || profile::is_root() {
        &profiles
    } else {
        mine = Profiles::personal(&store)?;
        &mine
    };
    apply_config_with(&store, target, &file, locked, upgrade, dry_run)?;
    Ok(())
}

/// `nami status`.
pub(crate) fn status(store: &Store, profiles: &Profiles, json: bool) -> Result<()> {
    let index_file = store.var_dir().join("index.json");
    // (One that came with the system has the image's files' date, 1970: none to say.)
    let catalog_time = std::fs::metadata(&index_file).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).filter(|d| d.as_secs() > 86400 * 365).map(|d| profile::format_time(d.as_secs()));
    let (packages, installable) = Index::load(&store).ok().map(|i| (i.len(), i.installable())).unzip();
    let installable = installable.flatten();
    // Nami files changed since they were applied: what each was then.
    let personal = Profiles::personal(&store).ok();
    let mut pending: Vec<serde_json::Value> = Vec::new();
    for p in [Some(profiles), personal.as_ref()].into_iter().flatten() {
        let Some(snap) = p.current().ok().flatten().and_then(|m| m.config) else { continue };
        let now = std::fs::read_to_string(&snap.file).ok();
        if now.as_deref() != Some(snap.text.as_str()) && !pending.iter().any(|v| v["path"] == snap.file.as_str()) {
            pending.push(serde_json::json!({ "path": snap.file, "applied": snap.text }));
        }
    }
    let v = serde_json::json!({
        "pending": pending,
        "from_stick": crate::nami_os::plugin().is_some_and(|os| os.started_from_stick()),
        "nami_os": crate::nami_os::here(),
        "user": std::env::var("USER").unwrap_or_default(),
        "system": profiles.is_system(),
        "generation": profiles.current_generation(),
        "installed": profiles.current_packages()?.len(),
        "catalog": { "packages": packages, "installable": installable, "updated": catalog_time },
        "daemon": store.daemon().is_some(),
        "admin": profile::is_admin() || profile::is_root(),
        "config": default_config(),
        "personal": personal_config(),
        "services": services_status(&profiles)?,
        "files": if profiles.is_system() { etc::status(store.root(), &store.var_dir()) } else { Vec::new() }
            .into_iter().map(|(path, changed)| serde_json::json!({ "path": path, "changed": changed })).collect::<Vec<_>>(),
    });
    if json {
        println!("{v}");
    } else {
        // A card: who it's for, then a line each, ✓ fine, ⚠ worth doing something about, ✗ wrong.
        let paint = style::out();
        let now = tell::now();
        let mut card: Vec<String> = Vec::new();
        let installed = v["installed"].as_u64().unwrap_or(0) as usize;
        match profiles.current_generation() {
            Some(g) => card.push(format!("✓ Change {g} is in use · {}", tell::count(installed, "app", "apps"))),
            None if profiles.is_system() => card.push("Nothing installed for the system yet · nami install --system <app>".into()),
            None => card.push("⚠ Nothing installed yet · nami install <app>".into()),
        }
        let catalog_secs = std::fs::metadata(&index_file).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).filter(|s| *s > 86400 * 365);
        match (packages, catalog_secs) {
            (None, _) => card.push("✗ No catalog yet: nothing to install from · nami update --refresh".into()),
            (Some(n), Some(t)) if now.saturating_sub(t) > 30 * 86400 => card.push(format!("⚠ Catalog · {} · from {} · nami update --refresh", tell::catalog_size(n, installable), tell::ago(t, now))),
            (Some(n), Some(t)) => card.push(format!("✓ Catalog · {} · updated {}", tell::catalog_size(n, installable), tell::ago(t, now))),
            (Some(n), None) => card.push(format!("✓ Catalog · {} · as it came with the system", tell::catalog_size(n, installable))),
        }
        for f in &pending {
            let path = f["path"].as_str().unwrap_or("");
            let name = std::path::Path::new(path).file_name().map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
            // (The computer's is sudo's to apply.)
            let apply = if crate::cmd::files::is_system_config(std::path::Path::new(path)) { "sudo nami apply" } else { "nami apply" };
            card.push(format!("⚠ {name} has changes not applied yet · {apply}"));
        }
        for s in v["services"].as_array().into_iter().flatten() {
            let unit = s["unit"].as_str().unwrap_or("");
            let state = s["state"].as_str().unwrap_or("");
            let journal = if s["user"].as_bool() == Some(true) { format!("journalctl --user -u {unit}") } else { format!("journalctl -u {unit}") };
            card.push(match state {
                _ if s["enable"].as_bool() == Some(false) => format!("✓ {unit} · {state} · not started by itself"),
                "active" => format!("✓ {unit} is running"),
                "failed" => format!("✗ {unit} failed · its log: {journal}"),
                "activating" | "reloading" => format!("⚠ {unit} is starting"),
                other => format!("⚠ {unit} isn't running ({other}) · its log: {journal}"),
            });
        }
        for f in v["files"].as_array().into_iter().flatten() {
            if f["changed"].as_bool() == Some(true) {
                card.push(format!("⚠ {} was changed by hand · nami apply puts it back", f["path"].as_str().unwrap_or("")));
            }
        }
        let files = v["files"].as_array().map_or(0, |a| a.iter().filter(|f| f["changed"].as_bool() != Some(true)).count());
        if files > 0 {
            card.push(format!("✓ {} in /etc as system.nami says", tell::count(files, "file", "files")));
        }
        let me = v["user"].as_str().filter(|u| !u.is_empty()).unwrap_or("you").to_string();
        let who = |system: bool| if system { "System · for everyone on this computer".to_string() } else { format!("{me} · only you") };
        let say = |who: String, card: &[String]| {
            let (head, place) = who.split_once(" · ").unwrap_or((&who, ""));
            println!("{} {}", paint.bold(head), paint.dim(&format!("· {place}")));
            for l in card {
                println!("{}", paint.line(&format!("  {l}")));
            }
        };
        say(who(profiles.is_system()), &card);
        // And the other one's change, said whose: yours, or the computer's (root has only it).
        let other = if profiles.is_system() { personal.filter(|p| !p.is_system()) } else { Some(Profiles::system(store)) };
        if let Some(o) = other {
            let line = match o.current_generation() {
                Some(g) => format!("✓ Change {g} is in use · {}", tell::count(o.current_packages().map(|p| p.len()).unwrap_or(0), "app", "apps")),
                None if o.is_system() => "Nothing installed for everyone yet".to_string(),
                None => "⚠ Nothing installed yet · nami install <app>".to_string(),
            };
            println!();
            say(who(o.is_system()), &[line]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One user.nami for a Mac and Linux: Linux leaves out Homebrew's casks, said in a line,
    /// and keeps the rest (as a Mac leaves out Flathub's apps: mac_only).
    #[test]
    fn linux_leaves_out_casks() {
        let text = "packages:\n  - ripgrep\n  - cask:rectangle\n  - flathub:org.gnome.Calculator\n  - cask:google-chrome\n";
        let config = config::load_text(text, "user.nami", std::path::Path::new(".")).unwrap();
        let (config, wanted) = casks_of(config, None, std::path::Path::new("user.nami"), &[("cask:nosuch".to_string(), String::new())]);
        if index::MAC {
            assert_eq!(wanted.iter().map(|w| w.token.as_str()).collect::<Vec<_>>(), ["rectangle", "google-chrome"]);
        } else {
            assert!(wanted.is_empty());
        }
        assert_eq!(config.packages, ["ripgrep", "flathub:org.gnome.Calculator"]);
    }
}
