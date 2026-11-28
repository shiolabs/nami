// (Without namiOS's part, what only it uses goes unused: src/nami_os.rs.)
#![cfg_attr(not(namios), allow(dead_code))]

mod cli;
mod cmd;
mod arch;
mod archive;
mod auto_update;
mod backup;
mod build;
mod build_errors;
mod build_kinds;
mod bundle;
mod bundle_app;
mod bundle_delta;
mod bundle_format;
mod bundle_menu;
mod bundle_publish;
mod bundle_sandbox;
mod bundle_test;
mod bundle_trust;
mod bundle_update;
mod catalog;
mod cleanup;
mod complete;
mod config;
mod doctor;
mod config_edit;
mod daemon;
mod deb;
mod elf;
mod errors;
mod flake;
mod casks;
mod flathub;
mod gitref;
mod foreign;
mod desktop;
mod desktop_ctl;
mod drift;
mod duration;
mod etc;
mod file_diff;
mod flat;
mod format;
mod hardware;
mod help;
mod home;
mod import;
mod index;
mod index_file;
mod layers;
mod lock;
mod log;
mod machine;
mod mac_apps;
mod new;
mod oldnames;
mod ops;
mod query;
mod recipe;
mod replace;
mod rpm;
mod schema;
mod report;
mod nar;
mod migrate;
mod narinfo;
mod nix;
mod openpgp;
mod other_linux;
mod profile;
mod proposals;
mod mcp;
mod nami_os;
// namiOS's part, built in when its folder is here (build.rs, src/nami_os.rs).
#[cfg(namios)]
#[path = "../namios/src/mod.rs"]
mod namios;
mod project;
mod push;
mod python_env;
mod secrets;
mod settings;
mod setup_sync;
mod site;
mod runtime;
mod vendor;
mod run_once;
mod selfupdate;
mod services;
mod show;
mod storage;
mod store;
mod style;
mod sys;
mod tell;
mod up;
mod wifi;
mod yaml_text;
mod zsync;

use std::path::PathBuf;

use anyhow::Result;

use profile::Profiles;
use store::Store;
use cli::{CatalogCommand, Cli, Command, Parsed, ProjectCommand, RecipeCommand};
use cmd::files::CHILD_APPS;
use cmd::packages::upgrade_flathub;
use cmd::apply::layers_command;
use cmd::config::config_command;
use cmd::generations::{clean_up_mine_daily, cleans_up_mine, gc_auto};
use cmd::update::NAMI_IS_FRESH;
use cmd::daemon::{changes_own_apps, changes_system, for_whom};
use cmd::propose::{Waiting, proposals_command, propose_command};
use cmd::catalog::nix_from_catalog;
use cmd::desktop::home_command;
use cmd::misc::secret_command;

fn main() {
    // `nami search | head`: a closed pipe ends nami quietly, as it does other tools.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    // What the shell's tab completion asks (src/complete.rs): first, and quick.
    if std::env::args().nth(1).as_deref() == Some("__complete") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        complete::main(cli::command(), &args);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("__complete-fetch") {
        complete::fetch(&std::env::args().skip(2).collect::<Vec<_>>());
        return;
    }
    tell::start();
    // tray's TRAY_* variables, read as NAMI_* (until 1.0).
    oldnames::environment();
    // --json: errors as JSON too (`errors`), the command line's own as well.
    let args: Vec<String> = std::env::args().collect();
    let json = errors::wants_json(&args);
    errors::set_json(json);
    // (Before anything's said, help included.)
    let choice = help::color_arg(&args);
    style::set_choice(choice);
    // `nami` alone, and `nami help`: nami's own screens (src/help.rs).
    if let Some(screen) = help::screen(&args) {
        let cmd = cli::command();
        match screen {
            help::Screen::Welcome => print!("{}", help::welcome(style::out(), &cmd, !config::user_file().exists())),
            help::Screen::Help { all } => print!("{}", help::top(style::out(), &cmd, all)),
            help::Screen::Json => println!("{}", serde_json::to_string_pretty(&help::json(&cmd)).unwrap_or_default()),
        }
        return;
    }
    let parsed = help::command(choice).try_get_matches_from(&args).and_then(Parsed::from_matches);
    let cli = match parsed {
        Ok(cli) => cli,
        // namiOS's commands, from a nami built without namiOS's part.
        Err(_) if nami_os::without(&args).is_some() => {
            let said = nami_os::without(&args).unwrap_or_default();
            if json {
                println!("{}", errors::shape(errors::Code::InvalidArgs, &said, None));
            } else {
                eprintln!("{}", style::err().line(&format!("error: {said}")));
            }
            std::process::exit(2);
        }
        Err(e) if !json && help::error(style::err(), &e).is_some() => {
            eprint!("{}", help::error(style::err(), &e).unwrap_or_default());
            std::process::exit(2);
        }
        Err(e) if json && !matches!(e.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) => {
            let message = e.to_string();
            let first = message.lines().next().unwrap_or_default().trim_start_matches("error: ").to_string();
            println!("{}", errors::shape(errors::Code::InvalidArgs, &first, Some("See the command's --help.")));
            eprint!("{message}");
            std::process::exit(2);
        }
        Err(e) => e.exit(),
    };
    style::set_choice(cli.color());
    if let Parsed::Nami(Cli { command: Command::Completions { shell }, .. }) = &cli {
        print!("{}", complete::script(shell).unwrap_or_default());
        return;
    }
    // What only changes things says {"ok": true} once it has, with --json.
    let says_ok = json && matches!(&cli, Parsed::Nami(Cli { command: Command::Install { .. } | Command::Remove { .. } | Command::Rollback { .. } | Command::Update { .. } | Command::Apply { dry_run: false, .. }, .. }));
    let ran = run(cli);
    if ran.is_ok() && says_ok {
        println!("{}", serde_json::json!({ "ok": true }));
    }
    if let Err(e) = ran {
        log::settle();
        let text = config::plain_message(&format!("{e:#}"));
        if json {
            println!("{}", errors::json_of(&e));
        }
        if style::is_said_as_check(&text) {
            // A file's problems, as `nami check` says them: each its own line.
            for l in text.lines() {
                eprintln!("{}", style::err().line(l));
            }
        } else {
            // In plain words (not `(os error 2)`), then what to do about it.
            let (text, hint) = errors::plain(&text, errors::classify(&e).1);
            eprintln!("{}", style::err().line(&format!("error: {text}")));
            if let Some(h) = hint.filter(|_| !json) {
                eprintln!("{}", style::err().line(&format!("hint: {h}")));
            }
        }
        // 75 (EX_TEMPFAIL): it's the network, so trying again later may
        // work (a first login does); 77 an admin's needed; 2 the command
        // line; 1 for the rest (`errors::Code::exit_code`).
        std::process::exit(errors::classify(&e).0.exit_code());
    }
}

fn run(cli: Parsed) -> Result<()> {
    // NAMI_ROOT lets tests use a scratch directory instead of `/`.
    let root = PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into()));
    // On namiOS: not as root without namiOS's part (src/nami_os.rs).
    if root == std::path::Path::new("/") {
        nami_os::refuse_without()?;
    }
    // tray's places become nami's (docs/rename-migration.md): the computer's
    // when root runs nami, the person's own as them. Not for the tools that
    // only make files (a release's build, the docs' data), which run anywhere.
    let makes_only = matches!(
        &cli,
        Parsed::Nami(Cli { command: Command::AttachBundleRuntime { .. } | Command::Options { .. } | Command::Keygen { .. } | Command::Catalog { .. } | Command::Build { .. } | Command::Bundle { .. } | Command::Recipe { .. }, .. })
    );
    if root == std::path::Path::new("/") && !makes_only {
        migrate::computer(&root);
        migrate::person();
    }
    // A flake's package (github:owner/repo#name): read with Nix from the
    // catalog when the computer has none, fetched only if one comes up.
    nix_from_catalog(&root, "to read a flake");
    // namiOS's commands: each run where nami's own would be.
    let cli = match cli {
        Parsed::NamiOs(m) => return nami_os::require()?.run(&root, &m),
        Parsed::Nami(cli) => cli,
    };
    // From files alone, on any computer (a GitHub runner's): no store.
    if let Command::Catalog { command: CatalogCommand::CaskIcons { casks, installs, top, also } } = &cli.command {
        return cmd::catalog::cask_icons(casks.clone(), installs.clone(), *top, also.clone());
    }
    // Layers from repos stay at their pinned commits, unless asked to move.
    match &cli.command {
        Command::Apply { locked: true, .. } | Command::SystemUse { locked: true, .. } | Command::OsUpdate { .. } | Command::OsUpdateCasks => layers::set_mode(layers::Mode::Locked),
        Command::Apply { upgrade: true, .. } | Command::SystemUse { upgrade: true, .. } | Command::Layers { update: true, .. } => layers::set_mode(layers::Mode::Update),
        // A project's: `--locked` (CI) refuses what isn't pinned, `--upgrade` moves the pins.
        Command::Shell { locked: true, .. } | Command::Run { locked: true, .. } | Command::Env { locked: true, .. } | Command::Up { locked: true, .. } => layers::set_mode(layers::Mode::Locked),
        Command::Shell { upgrade: true, .. } => layers::set_mode(layers::Mode::Update),
        // At every prompt, and in a list: never the network.
        Command::HookEnv { .. } | Command::Project { command: ProjectCommand::List { .. } } => layers::set_mode(layers::Mode::Offline),
        _ => {}
    }
    // A person's own old changes, as their `cleanup:` says: once a day, as they use nami
    // (the daily update does it too, as them).
    if !profile::is_root() && cleans_up_mine(&cli.command) {
        clean_up_mine_daily(&root);
    }
    // --user is your own file: as root (sudo) that would be root's, not yours.
    if says_user(&cli.command) && profile::is_root() {
        anyhow::bail!("--user is your own file, ~/user.nami: run it without sudo");
    }
    let cli = match cli.command {
        // (Done above: as root on `/`, the computer's places, and the person's own.)
        // (The installers': and nami's folder made, where it goes, on a new computer.)
        Command::Migrate => {
            if root == std::path::Path::new("/") && profile::is_root() {
                migrate::make(&root).map_err(|e| anyhow::anyhow!("making nami's folder: {e}"))?;
            }
            return Ok(());
        }
        // The daily cleanup: root's for everyone, anyone else's for their own (no daemon).
        Command::Gc { auto: true, json, .. } => return gc_auto(&root, json),
        Command::Backup { command } => return backup::cli(command),
        Command::Sync { command } => return setup_sync::cli(command),
        Command::Desktop(args) => return desktop_ctl::cli(args),
        // A bundle from where it updates (`nami install bundle:github:ada/myapp`): each person's own.
        Command::Install { packages, .. } if !packages.is_empty() && packages.iter().all(|p| bundle_app::is_entry(p)) => return cmd::bundle::install_bundles(packages),
        // Downloads (.deb, .rpm, AppImage, tarball): each person's own, as them, no daemon.
        Command::Install { packages, .. } if !packages.is_empty() && packages.iter().all(|p| foreign::is_package_file(p) || bundle_app::is_bundle_file(p)) => {
            return bundle_app::install_cli(&root, &packages, std::env::var_os("NAMI_NO_LIBRARIES").is_none());
        }
        Command::Remove { packages, .. } if !packages.is_empty() && !profile::is_root() && packages.iter().all(|p| foreign::find_installed(&foreign::Places::here(), p).is_some()) => return cmd::bundle::remove_downloads(packages),
        // A nami bundle: a one-file app (src/bundle_app.rs).
        Command::PackageInfo { file, json } if bundle_app::is_bundle_file(&file.to_string_lossy()) => return cmd::bundle::bundle_info(file, json),
        Command::OpenPackage { file } if bundle_app::is_bundle_file(&file.to_string_lossy()) => std::process::exit(bundle_app::open(&file)?),
        Command::PackageInfo { file, json } => return cmd::bundle::package_info(file, json),
        Command::OpenPackage { file } => std::process::exit(foreign::open(&file)?),
        Command::FileApps { check: Some(file), .. } => return foreign::update::check_download(&file),
        Command::FileApps { automatic: Some(on), app, .. } => {
            let app = app.ok_or_else(|| anyhow::anyhow!("which app? `nami file-apps --automatic {on} <app>`"))?;
            return foreign::update::set_automatic(&foreign::Places::here(), &app, on == "on");
        }
        Command::FileApps { update: true, app, background, .. } => return foreign::update::run(&root, &foreign::Places::here(), app.as_deref(), background),
        Command::FileApps { json, updates, .. } => return foreign::list(json, updates),
        Command::Layers { file, update, json } if !changes_system(&Command::Layers { file: file.clone(), update, json }) || profile::is_root() || !profile::is_admin() => {
            return layers_command(file, update, json);
        }
        Command::Daemon => {
            anyhow::ensure!(profile::is_root(), "the daemon must run as root");
            return daemon::serve(&root);
        }
        Command::Secret { command } => return secret_command(command),
        Command::Try { update_table: true, .. } => return run_once::update_cli(&root),
        Command::Try { package, not_found, command, .. } => return cmd::misc::run_once(&root, package, not_found, command),
        Command::Hook { shell } => {
            print!("{}", project::hook_script(&shell)?);
            return Ok(());
        }
        Command::HookEnv { fish } => return cmd::project::hook_env(&root, fish),
        Command::New { folder, from, force, json } => return cmd::project::new(&root, folder, from, force, json),
        Command::Project { command: ProjectCommand::List { json } } => return cmd::project::list_projects(&root, json),
        Command::Project { command: ProjectCommand::Tasks { json } } => return cmd::project::tasks(json),
        Command::Project { command: ProjectCommand::Services { json } } => return cmd::project::services(json),
        // (A Mac: not yet. They're supervised with Linux's process tools.)
        Command::Up { .. } | Command::Down { .. } | Command::Logs { .. } if index::MAC => {
            anyhow::bail!("a project's services (nami up) aren't on a Mac yet: start them yourself, in `nami shell`")
        }
        // Not under the store's lock while they run: only to get them ready.
        Command::Up { names, detach, project: p, locked } => return cmd::project::up(&root, names, detach, p, locked),
        Command::Down { names, project: p } => return cmd::project::down(names, p),
        Command::Logs { name, project: p } => return cmd::project::logs(name, p),
        Command::Config { command, file, system, .. } => return config_command(&root, command, file.or_else(|| system.then(cmd::files::system_config))),
        Command::Propose { edit, reason, by, wait, timeout, json, quiet } => return propose_command(&root, &edit, &reason, by.as_deref(), Waiting { wait, timeout, json, quiet }),
        Command::Proposals { command } => return proposals_command(&root, command),
        Command::Mcp => return mcp::serve(),
        // Reading only: no daemon, no lock (the catalog, if there is one, to check names).
        Command::Check { file, base, json } => return cmd::apply::check(&root, file, base, json),
        Command::Show { file, json, .. } => return cmd::apply::show(&root, file, json),
        Command::Options { json, schema } => return cmd::apply::options(json, schema),
        // A config repo: cloned as you, then applied by nami itself.
        Command::Apply { file: Some(f), locked, upgrade, dry_run, .. } if f.to_str().is_some_and(machine::is_repo) => {
            return machine::run(&root, f.to_str().unwrap(), machine::Flags { locked, upgrade, dry_run });
        }
        // Text work: no lock, no daemon; the catalog only to check names.
        Command::Import { path, to, file, dry_run, json, out, user, repo, recipes, home, only } => return cmd::misc::import(&root, path, to, file, dry_run, json, out, user, repo, recipes, home, only),
        // Programs built for other distributions: as you, no daemon (adding goes through `nami apply`).
        Command::Libraries { program, add, after_failure } => std::process::exit(other_linux::libraries(&root, &program, add, after_failure)?),
        Command::AttachBundleRuntime { nami, runtime, net, arm, arm_net } => return bundle::attach_runtime(&nami, &runtime, net.as_deref(), arm.as_deref(), arm_net.as_deref()),
        Command::Bundle { show_signing_key: true, .. } => return bundle_publish::show_key(),
        Command::Bundle { rotate_key: true, .. } => return bundle_publish::rotate_key(),
        Command::Bundle { verify: Some(file), .. } => return cmd::bundle::verify(file),
        Command::Start { file, args } => std::process::exit(other_linux::start(&root, &file, &args)?),
        Command::AddToLauncher { file } => return other_linux::add_to_launcher(&file).map(|_| ()),
        // Only /sys and the examples: no store, no daemon (nami-gui asks at start).
        Command::Doctor { json, about: true, .. } => return cmd::misc::doctor_about(json),
        Command::Doctor { json, hardware: true, .. } => return cmd::misc::doctor_hardware(json),
        // Reading only: the catalog, straight from the store (no daemon).
        Command::Info { packages, json, versions: true, all, .. } | Command::Versions { packages, json, all } => return cmd::packages::versions(&root, packages, json, all),
        Command::Info { packages, json, screenshots, all, .. } => return cmd::packages::info(&root, packages, json, screenshots, all),
        Command::Gc { old, keep, older_than, now, dry_run: true, mine, json, .. } => return cmd::generations::gc_dry_run(&root, old, keep, older_than, now, mine, json),
        Command::Storage { json } => return cmd::generations::storage(&root, json),
        Command::Search { term, all, category, json, flathub: with_flathub } => return cmd::packages::search(&root, term, all, category, json, with_flathub),
        // Without nami's lock: it runs nami doctor and nami history.
        Command::Report { message, yes, no_files, show, json } => return cmd::misc::send_report(&root, message, yes, no_files, show, json),
        // Without holding nami's lock: installing the new release runs its
        // setup.sh, whose own nami commands need the lock.
        Command::SelfUpdate { check, json } if profile::is_root() || !profile::is_admin() || check => return cmd::misc::self_update(&root, check, json),
        // nami itself, by its uninstaller: without nami's lock (it runs `nami let-go`), or the daemon.
        Command::SelfUninstall { yes, packages } => return cmd::self_uninstall::run(&root, yes, packages),
        // (`nami remove`, or `nami uninstall`, with nothing named.)
        Command::Remove { packages, .. } if packages.is_empty() => {
            return Err(errors::coded_hint(errors::Code::InvalidArgs, "which apps? `nami remove <name>`", format!("To uninstall nami itself: {}", cmd::self_uninstall::COMMAND)));
        }
        // The timer, as system.nami says (install.sh on another distribution).
        Command::OsUpdate { set_up: true } => {
            anyhow::ensure!(profile::is_root(), "nami os-update --set-up is for install.sh, as root");
            return auto_update::set_up_as_file_says(&root);
        }
        // The daily update, first of all nami itself, then the rest by the new one.
        Command::OsUpdate { set_up: false } if profile::is_root() && std::env::var_os(NAMI_IS_FRESH).is_none() => {
            // (A Mac's daily job whose file went while it was loaded: it ends itself.)
            if !auto_update::job_still_wanted() {
                return Ok(());
            }
            cmd::update::update_nami_first(&root)?;
            Cli { color: style::Choice::Auto, command: Command::OsUpdate { set_up: false } }
        }
        other => Cli { color: style::Choice::Auto, command: other },
    };
    let cli = for_whom(cli, &root)?;
    // Admins, the first person and someone unlocked: through the daemon (cmd/daemon.rs).
    cmd::daemon::forward(&cli.command, &root)?;
    // A child's apps are the computer's: an admin chooses them.
    if profile::is_child() && changes_own_apps(&cli.command) {
        anyhow::bail!("{CHILD_APPS}");
    }
    let store = Store::open(&root)?;
    // Flathub's apps alone: your own file's, an admin's too.
    let flathub_only = matches!(&cli.command, Command::Install { packages, .. } | Command::Remove { packages, .. } if !packages.is_empty() && packages.iter().all(|p| flathub::is_flathub(p)));
    // An app is the person's own unless it's the system's (`--system`, run as root).
    // Changes, and going back, are yours unless sudo (root) or --system says the system's.
    let own_app = matches!(&cli.command, Command::Gc { mine: true, .. } | Command::Install { system: false, .. } | Command::Remove { system: false, .. } | Command::Rollback { system: false, .. } | Command::Generations { system: false, .. } | Command::Diff { system: false, .. } | Command::Upgrade { system: false, .. });
    let system_side = matches!(&cli.command, Command::Upgrade { system: true, .. } | Command::Rollback { system: true, .. } | Command::Generations { system: true, .. } | Command::Diff { system: true, .. });
    let profiles = if system_side { Profiles::system(&store) } else if flathub_only || own_app { Profiles::personal(&store)? } else { Profiles::for_current_user(&store)? };

    match cli.command {
        Command::Home { command, config } => return home_command(&store, &Profiles::personal(&store)?, command, config),
        Command::Shell { dir, project: p, locked, upgrade, command } => cmd::project::shell(&store, dir, p, locked, upgrade, command)?,
        Command::Run { project: p, locked, command } => cmd::project::run(&store, p, locked, command)?,
        Command::Env { project: p, locked, fish } => cmd::project::env(&store, p, locked, fish)?,
        // Another distribution's: the system's apps at their newest (nami was first, above).
        Command::OsUpdate { .. } if !nami_os::here() => cmd::update::update_other_distribution(&store, &profiles)?,
        Command::OsUpdate { .. } => nami_os::require()?.daily_update(&store)?,
        // (As each person, from the daily update on a Mac.)
        Command::OsUpdateCasks => cmd::update::update_my_casks(&store)?,
        Command::LetGo => {
            anyhow::ensure!(profile::is_root() && !nami_os::here(), "nami let-go is for uninstall.sh, as root (on a computer nami didn't make)");
            Profiles::system(&store).let_go()?;
        }
        Command::Update { from: None, refresh: true, .. } => cmd::catalog::refresh(&store)?,
        Command::Update { from, drvs, icons, trust, unsigned, .. } => cmd::catalog::update(&store, &profiles, from, drvs, icons, trust, unsigned)?,
        Command::Export => cmd::apply::export(&profiles)?,
        Command::Apply { file, locked, upgrade, dry_run, password_stdin, .. } => cmd::apply::apply(&store, &profiles, file, locked, upgrade, dry_run, password_stdin)?,
        Command::Bundle { what, out, program, mesa, desktop, level, exclude, appimage, thin, name, app_version, icon, desktop_file, update_url, publish_dir, previous, developer, sandbox, test, test_on, test_args, json, arch, .. } => cmd::bundle::bundle(&store, what, out, program, mesa, desktop, level, exclude, appimage, thin, name, app_version, icon, desktop_file, update_url, publish_dir, previous, developer, sandbox, test, test_on, test_args, json, arch.unwrap_or_else(|| crate::arch::ARCH.to_string()))?,
        Command::Build { recipe, keep_failed } => cmd::catalog::build(&store, recipe, keep_failed)?,
        Command::Recipe { command: RecipeCommand::FromDeb { what, out, release } } => cmd::catalog::recipe_from_deb(&store, what, out, release)?,
        Command::Recipe { command: RecipeCommand::FromUrl { url, version, out } } => cmd::catalog::recipe_from_source(&store, "from-url", url, version, out)?,
        Command::Recipe { command: RecipeCommand::New { folder, out } } => cmd::catalog::recipe_from_source(&store, "new", folder.display().to_string(), None, out)?,
        Command::Recipe { command: RecipeCommand::Update { dir, version, refetch } } => cmd::catalog::recipe_update(&store, dir, version, refetch)?,
        Command::Catalog { command: CatalogCommand::Patch { old, new, out } } => cmd::catalog::patch(old, new, out)?,
        Command::Catalog { command: CatalogCommand::Sign { path, key } } => cmd::catalog::sign(&store, path, key)?,
        Command::Catalog { command: CatalogCommand::CaskIcons { casks, installs, top, also } } => cmd::catalog::cask_icons(casks, installs, top, also)?,
        Command::Keygen { name } => cmd::catalog::keygen(&store, name)?,
        Command::Push { dest, targets, key } => cmd::catalog::push(&store, &profiles, dest, targets, key)?,
        Command::Install { packages, .. } => cmd::packages::install(&store, &profiles, packages)?,
        Command::Remove { packages, .. } => cmd::packages::remove(&store, &profiles, packages)?,
        Command::Upgrade { flathub: true, dry_run: false, .. } => return upgrade_flathub(&store),
        Command::Upgrade { dry_run, json, flathub: only_flathub, .. } => cmd::packages::upgrade(&store, &profiles, dry_run, json, only_flathub)?,
        Command::List { json } => cmd::packages::list(&store, &profiles, json)?,
        Command::Status { json } => cmd::apply::status(&store, &profiles, json)?,
        // A computer nami made: the computer's steps are its systems (`nami apply` of system.nami).
        Command::Generations { json: true, .. } if profiles.is_system() && nami_os::here() => {
            println!("{}", serde_json::to_string(&nami_os::require()?.history(std::path::Path::new("/"))?)?);
        }
        Command::Generations { json: false, .. } if profiles.is_system() && nami_os::here() => {
            let mut steps = nami_os::require()?.history(std::path::Path::new("/"))?;
            steps.reverse();
            print!("{}", tell::timeline(&show::Term::out(), &steps, tell::now(), tell::undo_command(true)));
        }
        Command::Generations { json: true, .. } => println!("{}", serde_json::to_string(&query::history(&profiles)?)?),
        Command::Generations { json: false, .. } => {
            print!("{}", tell::timeline(&show::Term::out(), &query::history(&profiles)?, tell::now(), tell::undo_command(profiles.is_system())));
        }
        Command::Diff { from, to, raw, json, .. } if profiles.is_system() && nami_os::here() => nami_os::require()?.diff(from, to, raw, json)?,
        Command::Diff { from, to, raw, json, .. } => cmd::generations::diff(&profiles, from, to, raw, json)?,
        Command::Rollback { generation, .. } if profiles.is_system() && nami_os::here() => return nami_os::require()?.undo(&store, &profiles, generation),
        Command::Rollback { generation, .. } => cmd::generations::rollback(&profiles, generation)?,
        Command::SelfUpdate { .. } | Command::SelfUninstall { .. } | Command::New { .. } | Command::Report { .. } | Command::Migrate => unreachable!("handled before the store is locked"),
        Command::Layers { file, update, json } => layers_command(file, update, json)?,
        Command::SystemUse { from, locked, upgrade, dry_run } => cmd::setup::system_use_command(&store, &profiles, from, locked, upgrade, dry_run)?,
        Command::SystemInit { from } => cmd::setup::system_init(&store, &profiles, from)?,
        Command::Doctor { json, .. } => cmd::misc::doctor(&store, &profiles, json)?,
        Command::Gc { old, keep, older_than, now, .. } => cmd::generations::gc(&store, &profiles, old, keep, older_than, now)?,
        Command::AttachBundleRuntime { .. } | Command::Completions { .. } | Command::Backup { .. } | Command::Sync { .. } | Command::Desktop(_) | Command::Mcp | Command::Proposals { .. } | Command::Propose { .. } | Command::Daemon | Command::Secret { .. } | Command::Config { .. } | Command::Check { .. } | Command::Options { .. } | Command::Show { .. } | Command::Import { .. } | Command::Libraries { .. } | Command::Start { .. } | Command::AddToLauncher { .. } | Command::PackageInfo { .. } | Command::OpenPackage { .. } | Command::FileApps { .. } | Command::Search { .. } | Command::Info { .. } | Command::Versions { .. } | Command::Project { .. } | Command::Hook { .. } | Command::Try { .. } | Command::HookEnv { .. } | Command::Up { .. } | Command::Down { .. } | Command::Logs { .. } | Command::Storage { .. } => unreachable!(),
    }
    Ok(())
}


/// The command says --user: yours, ~/user.nami (the default without sudo).
fn says_user(command: &Command) -> bool {
    matches!(command, Command::Apply { user: true, .. } | Command::Config { user: true, .. } | Command::Diff { user: true, .. }
        | Command::Upgrade { mine: true, .. } | Command::Generations { mine: true, .. } | Command::Rollback { mine: true, .. } | Command::Gc { mine: true, .. })
}
#[cfg(test)]
mod tests {
    #[test]
    fn network_errors_are_for_later() {
        assert!(crate::errors::offline("fetching https://x/menu: Dns Failed: resolve dns name 'x:443'"));
        assert!(crate::errors::offline("cloning http://10.0.2.2/cfg.git: fatal: unable to access: Failed to connect to 10.0.2.2 port 80"));
        assert!(!crate::errors::offline("user.nami:3: unknown key `pakages` (did you mean `packages`?)"));
    }

    use super::*;
    use std::path::PathBuf;
    use clap::Parser;
    use crate::cmd::files::{default_config, personal_config, system_config};
    use crate::cmd::packages::{install_split, remove_split};
    use crate::cmd::apply::check_default;
    use crate::cmd::daemon::changes_system;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn user_says_your_own_file_and_mine_still_does() {
        // clap's whole command tree is deep for a test thread's stack (debug build).
        std::thread::Builder::new().stack_size(64 << 20).spawn(user_says_your_own_file).unwrap().join().unwrap();
    }

    fn user_says_your_own_file() {
        let says = |a: &[&str]| says_user(&Cli::try_parse_from(a).unwrap().command);
        for a in [&["nami", "apply", "--user"][..], &["nami", "undo", "--user"], &["nami", "undo", "--mine"], &["nami", "history", "--user"],
            &["nami", "upgrade", "--user"], &["nami", "diff", "--user"], &["nami", "config", "get", "packages", "--user"]] {
            assert!(says(a), "{a:?}");
        }
        assert!(!says(&["nami", "apply"]) && !says(&["nami", "undo", "--system"]) && !says(&["nami", "undo", "--computer"]));
        assert!(Cli::try_parse_from(["nami", "apply", "--user", "--system"]).is_err(), "one or the other");
        assert!(Cli::try_parse_from(["nami", "undo", "--user", "--system"]).is_err());
        // --system, and --computer as scripts and older nami-guis say it.
        for flag in ["--system", "--computer"] {
            for c in [&["nami", "apply", flag][..], &["nami", "undo", flag], &["nami", "history", flag], &["nami", "diff", flag], &["nami", "upgrade", flag], &["nami", "config", "path", flag]] {
                assert!(Cli::try_parse_from(c).is_ok(), "{c:?}");
            }
        }
    }

    #[test]
    fn apps_are_yours_unless_they_set_up_the_system() {
        // For you: the catalog's apps, your recipes, Flathub's.
        let (everyone, mine) = install_split(strings(&["gimp", "docker", "flathub:org.gimp.GIMP", "steam", "./packages/mine", "ripgrep"]), false);
        assert_eq!(everyone, ["docker", "steam"]);
        assert_eq!(mine, ["gimp", "flathub:org.gimp.GIMP", "./packages/mine", "ripgrep"]);
        // --for-everyone: all of them, but Flathub's (always each person's).
        let (everyone, mine) = install_split(strings(&["gimp", "flathub:org.gimp.GIMP", "docker"]), true);
        assert_eq!(everyone, ["gimp", "docker"]);
        assert_eq!(mine, ["flathub:org.gimp.GIMP"]);
        // nami's window is the computer's: `nami install nami-gui` lists it in system.nami.
        let (everyone, mine) = install_split(strings(&["nami-gui", "gimp"]), false);
        assert_eq!((everyone, mine), (strings(&["nami-gui"]), strings(&["gimp"])));
    }

    #[test]
    fn removing_takes_an_app_from_where_it_is() {
        let mine = "packages:\n  - gimp\n  - firefox: \"150\"\n  - flathub:org.gimp.GIMP\n";
        let (everyone, yours) = remove_split(strings(&["gimp", "firefox", "vlc", "flathub:org.gimp.GIMP"]), mine, std::path::Path::new("/home/x"));
        assert_eq!(yours, ["gimp", "firefox", "flathub:org.gimp.GIMP"]);
        assert_eq!(everyone, ["vlc"]);
    }

    #[test]
    fn check_without_a_file_takes_the_nearest_project() {
        let dir = std::env::temp_dir().join(format!("nami-check-default-{}", std::process::id()));
        let deep = dir.join("app/src/bin");
        std::fs::create_dir_all(&deep).unwrap();
        // No project: system.nami, as before.
        assert_eq!(check_default(&deep), None);
        std::fs::write(dir.join("app/project.nami"), "packages:\n  - ripgrep\n").unwrap();
        assert_eq!(check_default(&dir.join("app")), Some(PathBuf::from("project.nami")));
        assert_eq!(check_default(&deep), Some(PathBuf::from("../../project.nami")));
        // A computer's or a person's file is no project's.
        std::fs::remove_file(dir.join("app/project.nami")).unwrap();
        std::fs::write(dir.join("app/user.nami"), "packages:\n  - ripgrep\n").unwrap();
        assert_eq!(check_default(&deep), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_for_everyone_is_a_flag() {
        // (On a stack of a size nami has: clap's unoptimised build of all of nami's commands
        // takes more than a test's thread has.)
        let parse = |a: &[&str]| {
            let a = strings(a);
            std::thread::Builder::new()
                .stack_size(16 << 20)
                .spawn(move || match Cli::try_parse_from(a).unwrap().command {
                    Command::Install { packages, system, .. } | Command::Remove { packages, system, .. } => (packages, system),
                    _ => unreachable!(),
                })
                .unwrap()
                .join()
                .unwrap()
        };
        assert_eq!(parse(&["nami", "install", "gimp"]), (strings(&["gimp"]), false));
        assert_eq!(parse(&["nami", "install", "--for-everyone", "gimp", "vlc"]), (strings(&["gimp", "vlc"]), true));
        assert_eq!(parse(&["nami", "install", "gimp", "--everyone"]), (strings(&["gimp"]), true));
        assert_eq!(parse(&["nami", "remove", "--system", "gimp"]), (strings(&["gimp"]), true));
        // Only for everyone does an admin's go through the daemon (as root).
        assert!(changes_system(&Command::Install { packages: strings(&["gimp"]), system: true, json: false }));
        assert!(!changes_system(&Command::Install { packages: strings(&["gimp"]), system: false, json: false }));
        assert!(!changes_system(&Command::Install { packages: strings(&["flathub:a.b"]), system: true, json: false }));
        assert!(!changes_system(&Command::Remove { packages: strings(&["gimp"]), system: false, json: false }));
        // Sudo decides: plain apply and undo are yours; --computer (or a file that's the
        // computer's) goes to the daemon.
        let apply = |file: Option<&str>, system: bool| Command::Apply { file: file.map(std::path::PathBuf::from), locked: false, upgrade: false, dry_run: false, password_stdin: false, json: false, system, user: false };
        assert!(!changes_system(&apply(None, false)));
        assert!(changes_system(&apply(None, true)));
        assert!(!changes_system(&apply(Some("/home/ada/user.nami"), false)));
        assert!(changes_system(&apply(Some(&system_config().display().to_string()), false)));
        assert!(!changes_system(&Command::Rollback { generation: None, json: false, system: false, mine: false }));
        assert!(!changes_system(&Command::Rollback { generation: Some(3), json: false, system: false, mine: true }));
        assert!(changes_system(&Command::Rollback { generation: None, json: false, system: true, mine: false }));
        // The daemon's password box names them, not the flag.
        assert_eq!(daemon::asks_password(&strings(&["install", "--system", "docker"])).as_deref(), Some("docker is being installed on the system, for everyone on this computer."));
    }

    #[test]
    fn apply_defaults_to_the_system_or_user_file_never_a_project() {
        // Whatever folder you're in: ./project.nami is a project's, or named explicitly.
        // Sudo decides: root's is the computer's, anyone else's (an admin too) their own.
        let file = default_config();
        assert_eq!(file, if crate::profile::is_root() { system_config() } else { personal_config() }, "{}", file.display());
        assert!(file.is_absolute() || std::env::var_os("HOME").is_none());
    }
}
