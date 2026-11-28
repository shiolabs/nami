//! Apps: `nami install`, `remove`, `upgrade`, `list`, `search`, `info` and `versions`.

use std::path::PathBuf;

use anyhow::Result;

use crate::build::Builder;
use crate::index::Index;
use crate::profile::{Entry, Profiles};
use crate::store::Store;
use crate::cmd::files::{base_of, ensure_system_config, ensure_user_config, is_system_config, personal_config, system_config};
use crate::cmd::apply::{apply_config, edit_and_apply, finish, report, said_file};
use crate::{casks, config, config_edit, errors, flathub, foreign, index, new, ops, profile, query, show, store, style, tell};

/// Which apps of an install go where: (for everyone, for you). Your own
/// unless it sets up the computer (a service, files in /etc: docker,
/// steam) or is nami's own window (nami-gui), which only everyone's can, or
/// you asked for everyone.
pub(crate) fn install_split(packages: Vec<String>, system: bool) -> (Vec<String>, Vec<String>) {
    // (Flathub's apps are always each person's.)
    packages.into_iter().partition(|p| !flathub::is_flathub(p) && (system || new::sets_up_system(p) || new::is_own_app(p)))
}

/// Which apps of a removal come off where: (everyone's, yours). Each from
/// where it is: yours when your user.nami lists it (or it's from
/// Flathub), else the computer's.
pub(crate) fn remove_split(packages: Vec<String>, mine: &str, dir: &std::path::Path) -> (Vec<String>, Vec<String>) {
    packages.into_iter().partition(|p| !flathub::is_flathub(p) && config::remove_package_in(mine, &config_name(p, dir), dir).is_empty() && !cask_listed(mine, p, dir))
}

/// On a Mac, whether `mine` lists `p` as the other name of the same cask (`cask:iterm2`, `iterm2`).
fn cask_listed(mine: &str, p: &str, dir: &std::path::Path) -> bool {
    let other = casks::token(p).map(String::from).unwrap_or_else(|| format!("{}{p}", casks::PREFIX));
    index::MAC && !config::remove_package_in(mine, &other, dir).is_empty()
}

/// How a package is named in a config at `config_dir`: catalog names as they
/// are, recipe paths relative to the config when they're inside it.
pub(crate) fn config_name(name: &str, config_dir: &std::path::Path) -> String {
    if !Builder::is_recipe(name) {
        return name.to_string();
    }
    let abs = std::fs::canonicalize(name).unwrap_or_else(|_| PathBuf::from(name));
    let dir = std::fs::canonicalize(config_dir).unwrap_or_else(|_| config_dir.to_path_buf());
    match abs.strip_prefix(&dir) {
        Ok(rel) => format!("./{}", rel.display()),
        Err(_) => abs.display().to_string(),
    }
}

/// Your apps from Flathub to Flathub's newest (Flatpak's runtimes with
/// them), as a step of your history when a version changed.
pub(crate) fn upgrade_flathub(store: &Store) -> Result<()> {
    let profiles = Profiles::personal(store)?;
    if profiles.is_system() {
        return Ok(());
    }
    let Some(m) = profiles.current()? else { return Ok(()) };
    let ids = flathub::ids_of(&m.packages);
    if ids.is_empty() {
        return Ok(());
    }
    flathub::Flatpak::find()?.update()?;
    flathub::link_services();
    let fresh = flathub::entries(&ids);
    let packages: Vec<Entry> = m.packages.iter().map(|e| fresh.iter().find(|f| f.attr == e.attr).cloned().unwrap_or_else(|| e.clone())).collect();
    let before = profiles.current_generation();
    let after = profiles.commit_with(None, packages, None, None, None, None)?;
    report(&profiles, before, after)?;
    if after.is_some() {
        tell::set_intent(tell::Intent::Upgrade);
        finish(std::path::Path::new("Flathub"), &profiles);
    }
    Ok(())
}

/// Whether a package kept under its own name is that version: python311
/// is 3.11.x, nodejs_22 22.x, go_1_26 1.26.x (go365 is another program).
/// What you have (your own profile) and what everyone has (the computer's),
/// to say where something's installed; what can't be read, left out.
/// On namiOS the system's apps are its system's (system.nami's), not a profile's.
fn installed_lists(store: &Store, root: &std::path::Path, index: &Index) -> (Vec<profile::Entry>, Vec<profile::Entry>) {
    let mut everyone = Profiles::system(store).current_packages().unwrap_or_default();
    if let Some((flat, roots)) = crate::nami_os::plugin().and_then(|os| os.current_system(root)) {
        for (name, _) in query::flat_packages(&flat) {
            let base = index::split_version(&name).0.to_string();
            // Its version: the system's store path of the same name as the catalog's.
            let pname = index.find(&base).map(|p| { let v = query::version_of(&p.name); p.name.strip_suffix(&format!("-{v}")).unwrap_or(&p.name).to_string() }).unwrap_or_else(|| base.clone());
            let name = roots
                .iter()
                .filter_map(|r| store::base_name(r).get(33..))
                .find(|n| n.strip_prefix(&pname).is_some_and(|rest| rest.starts_with('-') && rest[1..].starts_with(|c: char| c.is_ascii_digit())))
                .map_or_else(|| base.clone(), str::to_string);
            everyone.push(profile::Entry { attr: base, name, paths: Vec::new() });
        }
    }
    let user = std::env::var("USER").ok().filter(|u| !u.is_empty() && !u.contains('/') && !u.starts_with('.'));
    let mine = match user {
        Some(u) if !profile::is_root() => Profiles::user(store, &u).current_packages().unwrap_or_default(),
        _ => Vec::new(),
    };
    (mine, everyone)
}

/// Where `base` (a name without a version) is installed.
fn installed_place(base: &str, installed: bool, mine: &[profile::Entry], everyone: &[profile::Entry]) -> show::Place {
    let has = |list: &[profile::Entry]| list.iter().any(|e| index::split_version(&e.attr).0 == base);
    match (has(mine), has(everyone)) {
        (true, true) => show::Place::Both,
        (true, false) => show::Place::Yours,
        (false, true) => show::Place::Everyones,
        (false, false) if installed => show::Place::Other,
        _ => show::Place::None,
    }
}

/// `nami search firefox@150`: whether the catalog has that version, when it
/// was current, and how to have it.
fn search_version(index: &Index, base: &str, wanted: &str) -> Result<()> {
    let t = show::Term::out();
    let list = match index.versions(base) {
        Ok(l) => l,
        Err(_) if index.find(base).is_none() => {
            let close = config::typo_matches(index, base);
            print!("{}", show::not_a_package(&t, base, &close));
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let found = match index.get(&format!("{base}@{wanted}")) {
        Ok(p) => {
            let version = query::version_of(&p.name);
            // When it was current: in its own history, or a sibling's (python312's 3.12).
            let said = |r: &show::Release, as_: Option<&str>| {
                let named = as_.map(|s| format!(", as {s}")).unwrap_or_default();
                match r.until {
                    None if as_.is_some() => format!("In the catalog now{named}."),
                    None => format!("The catalog's own version, {}.", show::span(r.from.as_deref(), None)),
                    Some(_) => format!("Current {}{named}.", show::span(r.from.as_deref(), r.until.as_deref())),
                }
            };
            let mut when = show::releases(&list).iter().find(|r| r.version == version).map(|r| said(r, None));
            if when.is_none() {
                for s in index.siblings(base) {
                    let Ok(sl) = index.versions(s) else { continue };
                    if let Some(r) = show::releases(&sl).iter().find(|r| r.version == version) {
                        when = Some(said(r, Some(s)));
                        break;
                    }
                }
            }
            show::Found::Yes { version, when: when.unwrap_or_else(|| "From an earlier catalog.".into()) }
        }
        Err(_) => show::Found::No { lines: show::lines(&show::releases(&list)).into_iter().map(|l| l.key).collect() },
    };
    print!("{}", show::version_search(&t, base, wanted, &found));
    Ok(())
}

fn version_named(suffix: &str, version: &str) -> bool {
    let named: String = suffix.chars().filter(char::is_ascii_digit).collect();
    let digits: String = version.chars().filter(char::is_ascii_digit).collect();
    !named.is_empty() && suffix.trim_start_matches('_').chars().all(|c| c.is_ascii_digit() || c == '_') && digits.starts_with(&named)
}

#[cfg(test)]
mod version_named_tests {
    #[test]
    fn names_match_their_versions() {
        assert!(super::version_named("311", "3.11.17"));
        assert!(super::version_named("_22", "22.23.3"));
        assert!(super::version_named("_1_26", "1.26.8"));
        assert!(!super::version_named("365", "2.0"));
        assert!(!super::version_named("-lts", "1.0"));
    }
}

/// `nami versions`, and `nami info --versions`.
pub(crate) fn versions(root: &std::path::Path, packages: Vec<String>, json: bool, all: bool) -> Result<()> {
    let store = Store::open_direct_readonly(&root)?;
    let index = Index::load(&store)?;
    let (mine, everyone) = installed_lists(&store, &root, &index);
    let t = show::Term::out();
    let mut all_json = serde_json::Map::new();
    for (n, a) in packages.iter().enumerate() {
        // Flathub keeps only its newest; so do Homebrew's casks.
        let cask = casks::is_cask(a) || (index::MAC && index.find(a).is_none() && casks::find(a).is_some());
        if flathub::is_flathub(a) || cask {
            all_json.insert(a.clone(), serde_json::json!([]));
            if !json {
                let keeps = if cask { "Homebrew's casks have" } else { "Flathub keeps" };
                println!("{}{a}: {keeps} only its newest version.", if n > 0 { "\n" } else { "" });
            }
            continue;
        }
        let base = index::split_version(a).0;
        let list = index.versions(base)?;
        // Versions kept under their own names: `nodejs: 20` finds nodejs_20's.
        let siblings: Vec<(String, String)> = index
            .siblings(base)
            .into_iter()
            .filter_map(|s| index.get(s).ok().map(|p| (s.to_string(), query::version_of(&p.name))))
            .collect();
        if !json {
            let installed = mine.iter().chain(&everyone).find(|e| index::split_version(&e.attr).0 == base).map(|e| query::version_of(&e.name));
            let v = show::Versions { attr: base, list: &list, installed, siblings: siblings.clone(), all };
            print!("{}{}", if n > 0 { "\n" } else { "" }, show::versions(&t, &v));
        }
        let mut entries: Vec<serde_json::Value> = list.iter().map(|v| serde_json::json!({ "version": v.0, "date": v.1, "nixpkgs": v.2 })).collect();
        // And the ones kept under their own names (nodejs_22): `nodejs: 22` finds them.
        for (s, version) in siblings.into_iter().filter(|(s, v)| version_named(s.strip_prefix(base).unwrap_or(s), v)) {
            entries.push(serde_json::json!({ "version": version, "date": "", "nixpkgs": "", "attr": s }));
        }
        all_json.insert(a.clone(), serde_json::json!(entries));
    }
    if json {
        println!("{}", serde_json::Value::Object(all_json));
    }
    Ok(())
}

/// `nami info`.
pub(crate) fn info(root: &std::path::Path, packages: Vec<String>, json: bool, screenshots: bool, all: bool) -> Result<()> {
    let store = Store::open_direct_readonly(&root)?;
    let profiles = Profiles::for_current_user(&store)?;
    let index = Index::load(&store)?;
    let installed = profiles.current_packages()?;
    // (Everyone's, for a cask's icon: an app the computer has shows its own.)
    let everyone = if index::MAC { Profiles::system(&store).current_packages().unwrap_or_default() } else { Vec::new() };
    let bin = query::profile_bin(&profiles);
    let mut items: Vec<query::Item> = Vec::new();
    for a in &packages {
        if let Some(id) = flathub::id(a) {
            items.extend(flathub::info(id));
            continue;
        }
        // Homebrew's casks (a Mac's): `cask:<token>`, or a name nixpkgs has no ready-built Mac
        // build of that a cask has.
        let token = casks::token(a).or_else(|| (index::MAC && index.get(a).map_or(true, |p| !casks::ready_built(p))).then_some(a.as_str()));
        if let Some(item) = token.and_then(|t| casks::info(t, &installed, &everyone, Some(&index))) {
            items.push(item);
            continue;
        }
        if let Some(t) = casks::token(a) {
            let why = if index::MAC { format!("{t}{}", casks::NOT_A_CASK) } else { format!("{a}: Homebrew's casks are Mac apps") };
            if !json {
                eprintln!("{why}");
            }
            continue;
        }
        // A Python with packages (`python313 with numpy`): its Python's card, saying so.
        if let Some(spec) = crate::python_env::parse(a) {
            let pkg = index.get(&spec.python).ok();
            let name = installed.iter().find(|e| e.attr == *a).map(|e| e.name.clone()).or_else(|| pkg.map(|p| p.name.clone())).unwrap_or_default();
            let mut it = query::item(a, pkg, &name, &installed, &bin, Some(&index));
            it.description = Some(spec.about(&it.version));
            items.push(it);
            continue;
        }
        match index.get(a) {
            Ok(p) => items.push(query::item(a, Some(p), &p.name, &installed, &bin, Some(&index))),
            Err(e) if index::split_version(a).1.is_some() => eprintln!("{e:#}"),
            Err(_) if json => {}
            Err(_) => {
                let close = config::typo_matches(&index, a);
                eprint!("{}", show::not_a_package(&show::Term::err(), a, &close));
            }
        }
    }
    if screenshots {
        for i in &mut items {
            i.screenshots = query::fetch_screenshots(store.http(), &i.screenshots);
        }
    }
    if json {
        println!("{}", serde_json::to_string(&items)?);
    } else {
        let t = show::Term::out();
        let (mine, everyone) = installed_lists(&store, &root, &index);
        for (n, i) in items.iter().enumerate() {
            let base = index::split_version(&i.attr).0;
            let place = installed_place(base, i.installed, &mine, &everyone);
            // The version history: one small piece of it, fetched once (offline: not said).
            let history = i.source.is_none().then(|| index.versions(base).ok()).flatten().map(|l| show::history_summary(&l));
            let card = show::Card { item: i, place, history, all: all || screenshots };
            print!("{}{}", if n > 0 { "\n" } else { "" }, show::info(&t, &card));
        }
    }
    Ok(())
}

/// `nami search`.
pub(crate) fn search(root: &std::path::Path, term: String, all: bool, category: Option<String>, json: bool, with_flathub: bool) -> Result<()> {
    let store = Store::open_direct_readonly(&root)?;
    let profiles = Profiles::for_current_user(&store)?;
    let index = Index::load(&store)?;
    // `nami search firefox@150`: is there such a version?
    if let (base, Some(wanted)) = index::split_version(term.trim()) {
        if !json {
            return search_version(&index, base, wanted);
        }
    }
    let (mut items, mut hidden) = query::search_counting(&index, &term, all, category.as_deref(), &profiles, !json && !all)?;
    // No app by that name: it's probably a tool or library, so look there too.
    let widened = !all && items.is_empty();
    if widened {
        items = query::search(&index, &term, true, category.as_deref(), &profiles)?;
        hidden = 0;
    }
    if with_flathub && category.is_none() {
        flathub::add_to_search(&mut items, &term, 20);
    }
    // A Mac's apps: Homebrew's casks, after the catalog's.
    if index::MAC && category.is_none() {
        casks::add_to_search(&mut items, &term, 20, &index, &profiles.current_packages().unwrap_or_default(), &Profiles::system(&store).current_packages().unwrap_or_default());
    }
    if json {
        println!("{}", serde_json::to_string(&items)?);
    } else {
        let t = show::Term::out();
        if items.is_empty() {
            let close = if term.trim().is_empty() { Vec::new() } else { config::typo_matches(&index, &term.trim().to_lowercase()) };
            print!("{}", show::nothing_found(&t, term.trim(), category.as_deref(), &close, with_flathub));
            return Ok(());
        }
        if widened && items.iter().any(|i| i.source.is_none()) {
            eprintln!("{}", style::err().dim("No apps match; these are command-line tools and libraries."));
        }
        let (mine, everyone) = installed_lists(&store, &root, &index);
        let has = |i: &query::Item| i.installed || installed_place(index::split_version(&i.attr).0, false, &mine, &everyone) != show::Place::None;
        print!("{}", show::search(&t, &items, has, term.trim(), hidden));
    }
    Ok(())
}

/// `nami install`.
pub(crate) fn install(store: &Store, profiles: &Profiles, packages: Vec<String>) -> Result<()> {
    let managed = match profiles.managed_config()? {
        None if profiles.is_system() && profile::is_root() => Some(ensure_system_config(&profiles)?),
        None if !profiles.is_system() => Some(ensure_user_config(&profiles)?),
        m => m,
    };
    // (system.nami by the name `nami apply` says, /etc/nami/system.nami, not
    // where its folder is: /etc/nami-config/<host>.)
    let managed = managed.map(|f| if is_system_config(&f) { system_config() } else { f });
    if let Some(file) = managed {
        let dir = base_of(&file);
        // What sets up a computer nami made comes with its setup (nami's
        // package of its name, next to the computer's folder), as
        // Settings' add-ons do.
        if profiles.is_system() && crate::nami_os::here() {
            let setups: Vec<String> = packages.iter().filter(|p| new::sets_up_system(p)).cloned().collect();
            new::copy_packages(&dir, &setups)?;
        }
        // nami's own apps (nami-gui), which no catalog has: their recipe as
        // this nami has it, in the packages/ folder the name is looked up in.
        if profiles.is_system() {
            for p in packages.iter().filter(|p| new::is_own_app(p)) {
                let shared = config::packages_dir(&dir).unwrap_or_else(|| dir.join(config::PACKAGES_DIR));
                new::write_package(&shared, p)?;
            }
        }
        // A cask nami can't install yet (a .pkg, another macOS...) isn't written into the file:
        // said now, as it is.
        if index::MAC {
            let index = Index::load(&store).ok();
            for w in casks::want(&packages, index.as_ref(), |t| casks::known(t) == Some(true)) {
                if let Some(why) = casks::find(&w.token).and_then(|s| s.refusal.clone()) {
                    return Err(errors::coded_hint(errors::Code::Unsupported, format!("{}; nothing was changed", why.trim_end_matches('.')), "`nami search <word>` finds the apps nami can install."));
                }
            }
        }
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let listed = config::listed_packages(&text);
        let mut ops = Vec::new();
        for p in &packages {
            // Another version of it in the file: this one instead.
            let name = config_name(p, &dir);
            if !listed.iter().any(|(have, _)| config::same_package(have, &name, &dir)) {
                ops.extend(config::add_package(&text, &name)?);
            }
        }
        // A name the catalog doesn't have isn't written into the file at all (an apply
        // of a file would leave it out, and go on): said, as not found.
        if let Ok(index) = Index::load(&store) {
            let mut after = text.clone();
            for op in &ops {
                after = config_edit::apply(&after, op)?;
            }
            let tier = if profiles.is_system() { config::Tier::System } else { config::Tier::User };
            let before: Vec<String> = config::check_tier(&text, &dir, Some(&index), tier).into_iter().map(|d| d.message).collect();
            let new: Vec<config::Diagnostic> = config::check_tier(&after, &dir, Some(&index), tier).into_iter().filter(|d| !before.contains(&d.message)).collect();
            if let Some(d) = new.iter().find(|d| d.message.ends_with(" isn't in the catalog") || d.message.ends_with(casks::NOT_A_CASK)) {
                let name = d.message.trim_end_matches(" isn't in the catalog").trim_end_matches(casks::NOT_A_CASK);
                return Err(errors::coded_hint(errors::Code::NotFound, format!("{}; nothing was changed", d.message), d.hint.clone().unwrap_or_else(|| format!("`nami search {name}` looks for it."))));
            }
            // A cask at a version: Homebrew's casks have their newest alone.
            if let Some(d) = new.iter().find(|d| d.severity == config::Severity::Error && d.message.contains("Homebrew's casks have only")) {
                return Err(errors::coded_hint(errors::Code::NotFound, format!("{}; nothing was changed", d.message), d.hint.clone().unwrap_or_else(|| "`nami info cask:<token>` says its newest version.".into())));
            }
        }
        tell::set_intent(tell::Intent::Install { names: packages.clone(), file: said_file(&file, &profiles) });
        return edit_and_apply(&store, &profiles, &file, &ops, false);
    }
    let before = profiles.current_generation();
    let index = Index::load(&store)?;
    tell::set_intent(tell::Intent::Install { names: packages.clone(), file: "your profile".into() });
    report(&profiles, before, ops::install(&store, &profiles, &index, &packages)?)?;
    finish(std::path::Path::new("your apps"), &profiles);
    Ok(())
}

/// `nami remove`.
pub(crate) fn remove(store: &Store, profiles: &Profiles, packages: Vec<String>) -> Result<()> {
    let managed = match profiles.managed_config()? {
        None if profiles.is_system() && profile::is_root() && system_config().exists() => Some(system_config()),
        m => m,
    };
    if let Some(file) = managed {
        let dir = base_of(&file);
        let text = std::fs::read_to_string(&file)?;
        let mut ops = Vec::new();
        for p in &packages {
            // `firefox` removes it at whatever version the file asks for.
            let name = config_name(p, &dir);
            let mut these = config::remove_package_in(&text, &name, &dir);
            // A cask as the file has it: `cask:iterm2` for `iterm2`, and the other way round.
            if these.is_empty() && index::MAC {
                let other = casks::token(&name).map(String::from).unwrap_or_else(|| format!("{}{name}", casks::PREFIX));
                these = config::remove_package_in(&text, &other, &dir);
            }
            if these.is_empty() {
                return Err(errors::coded_hint(errors::Code::NotFound, format!("{name} isn't installed: {} doesn't list it, so there's nothing to remove", said_file(&file, &profiles)), "`nami list` shows what's installed."));
            }
            ops.extend(these);
        }
        tell::set_intent(tell::Intent::Remove { names: packages.clone(), file: said_file(&file, &profiles) });
        return edit_and_apply(&store, &profiles, &file, &ops, false);
    }
    let before = profiles.current_generation();
    tell::set_intent(tell::Intent::Remove { names: packages.clone(), file: "your profile".into() });
    report(&profiles, before, ops::remove(&profiles, &Index::load(&store)?, &packages)?)?;
    finish(std::path::Path::new("your apps"), &profiles);
    Ok(())
}

/// `nami upgrade`.
pub(crate) fn upgrade(store: &Store, profiles: &Profiles, dry_run: bool, json: bool, only_flathub: bool) -> Result<()> {
    let before = profiles.current_generation();
    let index = Index::load(&store)?;
    // (With Homebrew's casks, not those that update themselves.)
    let mut ups: Vec<query::Update> = if only_flathub { Vec::new() } else { ops::updates(&profiles, &index)? };
    // Your apps from Flathub (they're each person's: never the computer's).
    if dry_run && !profiles.is_system() {
        let mine = profiles.current()?.map(|m| m.packages).unwrap_or_default();
        ups.extend(flathub::updates(&mine).into_iter().map(|(attr, from, to)| query::Update { attr, from, to }));
    }
    if json && dry_run {
        println!("{}", serde_json::to_string(&ups)?);
        return Ok(());
    }
    if !dry_run {
        tell::set_intent(tell::Intent::Upgrade);
        if let Some(file) = profiles.managed_config()? {
            apply_config(&store, &profiles, &file, false, true)?;
            return upgrade_flathub(&store);
        }
    }
    let paint = style::out();
    if dry_run && ups.is_empty() {
        println!("{}", paint.line("✓ Everything is up to date"));
    } else if dry_run {
        println!("{}", paint.line("Updates"));
        for u in &ups {
            println!("{}", paint.line(&format!("  ~ {} {} → {}", u.attr, u.from, u.to)));
        }
        println!("{}", paint.line(&format!("Dry run · {} · nami upgrade", tell::count(ups.len(), "update", "updates"))));
    }
    if !dry_run {
        report(&profiles, before, ops::upgrade(&store, &profiles, &index)?)?;
        finish(std::path::Path::new("your apps"), &profiles);
        upgrade_flathub(&store)?;
    }
    Ok(())
}

/// `nami list`.
pub(crate) fn list(store: &Store, profiles: &Profiles, json: bool) -> Result<()> {
    let index = Index::load(&store).ok();
    let items = query::list(index.as_ref(), &profiles)?;
    if json {
        println!("{}", serde_json::to_string(&items)?);
        return Ok(());
    }
    let row = |i: &query::Item| {
        let (name, pin) = index::split_version(&i.attr);
        show::Row { name: name.to_string(), pin: pin.map(String::from), version: i.version.clone(), about: show::about(i) }
    };
    // Where each is from: system.nami, yours, Flathub, downloads.
    let mut groups: Vec<(String, Vec<show::Row>)> = Vec::new();
    let mut flathub = Vec::new();
    let mut split = |items: Vec<query::Item>| -> Vec<show::Row> {
        let (fl, rest): (Vec<_>, Vec<_>) = items.into_iter().partition(|i| i.source.as_deref() == Some("flathub") || flathub::is_flathub(&i.attr));
        flathub.extend(fl.iter().map(row));
        rest.iter().map(row).collect()
    };
    let mine_title = format!("Yours · {}", personal_config().file_name().map_or("user.nami".into(), |n| n.to_string_lossy().into_owned()));
    // Both, each said whose: the system's (for everyone on the computer), then yours.
    let computer = Profiles::system(&store);
    let (everyone_items, mine_items) = if profiles.is_system() {
        (items, if profile::is_root() { Vec::new() } else { query::list(index.as_ref(), &Profiles::personal(&store)?)? })
    } else {
        (query::list(index.as_ref(), &computer).unwrap_or_default(), items)
    };
    {
        let mut everyone = split(everyone_items);
        if everyone.is_empty() {
            // (On namiOS they're in the system, not a profile: as system.nami has them.)
            let listed = computer.managed_config().ok().flatten().or_else(|| Some(system_config()).filter(|f| f.exists())).and_then(|f| config::load(&f).ok()).map(|c| c.packages).unwrap_or_default();
            for p in listed {
                let pkg = index.as_ref().and_then(|i| i.get(&p).ok());
                let version = pkg.map(|p| p.name.rsplit_once('-').map(|(_, v)| v.to_string()).unwrap_or_default()).unwrap_or_default();
                let (name, pin) = index::split_version(&p);
                everyone.push(show::Row { name: name.to_string(), pin: pin.map(String::from), version, about: pkg.and_then(|p| p.description.clone()).unwrap_or_default() });
            }
        }
        groups.push(("System · system.nami".into(), everyone));
        if !profile::is_root() {
            let mine = split(mine_items);
            groups.push((mine_title, mine));
        }
    }
    groups.push(("Yours, from Flathub".into(), flathub));
    if !profile::is_root() {
        let downloads = foreign::records(&foreign::Places::here()).into_iter().map(|r| show::Row { name: r.name, pin: None, version: r.version, about: r.summary.unwrap_or(r.format) }).collect();
        groups.push(("Yours, downloaded · .deb, .rpm, AppImage".into(), downloads));
    }
    print!("{}", show::list(&show::Term::out(), &groups));
    Ok(())
}
