//! The rest: `nami doctor`, `report`, `self-update`, `secret`, `try` and `import`.

use std::path::PathBuf;

use anyhow::Result;

use crate::index::Index;
use crate::profile::Profiles;
use crate::store::Store;
use crate::cli::SecretCommand;
use crate::cmd::files::system_config;
use crate::cmd::catalog::nix_from_catalog;
use crate::{doctor, hardware, import, profile, recipe, report, run_once, runtime, secrets, selfupdate, services, style, tell};

/// Secrets work without root or the daemon: they only touch your files.
pub(crate) fn secret_command(command: SecretCommand) -> Result<()> {
    match command {
        SecretCommand::Seal { files } => secrets::seal(&files),
        SecretCommand::Edit { file } => secrets::edit(&file),
        SecretCommand::Show { file, key } => secrets::show(&file, key.as_deref()),
        SecretCommand::Allow { name, key, remove } => {
            if !secrets::allow(&name, key.as_deref(), remove, None)?.is_empty() {
                eprintln!("commit {} and the files, and push", secrets::RECIPIENTS);
            }
            Ok(())
        }
        SecretCommand::Key => secrets::key(),
        SecretCommand::Hook => secrets::hook(),
        SecretCommand::Check { files, staged } => {
            let problems = secrets::check(&files, staged)?;
            for p in &problems {
                eprintln!("{p}");
            }
            if !problems.is_empty() {
                let how = if staged { "`nami secret seal`, then `git add` the files again" } else { "`nami secret seal` seals them" };
                anyhow::bail!("{} secret{} not sealed: {how}", problems.len(), if problems.len() == 1 { "" } else { "s" });
            }
            Ok(())
        }
    }
}

/// `nami try`.
pub(crate) fn run_once(root: &std::path::Path, package: Option<String>, not_found: bool, command: Vec<String>) -> Result<()> {
    // (A package by itself: its main program.)
    let command = match (command.is_empty(), &package) {
        (true, Some(p)) => vec![run_once::main_program(&root, p)?],
        _ => command,
    };
    let Some((cmd, args)) = command.split_first() else { anyhow::bail!("what to run: `nami try cowsay hi`") };
    let code = if not_found { run_once::not_found(&root, cmd, args)? } else { run_once::run_once(&root, package.as_deref(), cmd, args)? };
    std::process::exit(code);
}

/// `nami import`.
pub(crate) fn import(root: &std::path::Path, path: PathBuf, to: Option<import::Tier>, file: Option<PathBuf>, dry_run: bool, json: bool, out: Option<PathBuf>, user: Option<String>, repo: Option<String>, recipes: bool, home: Option<String>, only: Option<Vec<String>>) -> Result<()> {
    let index = Store::open_direct_readonly(&root).ok().and_then(|s| Index::load(&s).ok());
    // A whole computer: a config repo.
    if import::moving::what(&path).is_some() {
        anyhow::ensure!(to.is_none() && file.is_none() && (!json || home.is_some()), "moving from a computer writes a config repo (--out), not one file");
        anyhow::ensure!(only.is_none() || home.as_deref().is_some_and(|h| !h.is_empty()), "--only picks which of your files --home <folder> copies");
        let make = |what: &str, dir: &std::path::Path| -> Result<()> {
            let store = Store::open(&root)?;
            let index = Index::load(&store)?;
            recipe::from_deb(&store, &index, what, Some(dir), recipe::RELEASE).map(|_| ())
        };
        return import::moving::run(import::moving::Options { root: path, out, user, repo, recipes, home, only, json, dry_run }, index.as_ref(), Some(&make));
    }
    anyhow::ensure!(out.is_none() && user.is_none() && repo.is_none() && !recipes && home.is_none() && only.is_none(), "--out, --user, --repo, --recipes, --home and --only are for moving from a computer (`nami import /`)");
    // No Nix on this computer: a flake is evaluated with Nix from the
    // catalog, fetched like any package (through the daemon).
    nix_from_catalog(&root, "to read the flake");
    import::run(import::Options { path, to, file, dry_run, json }, index.as_ref())
}

/// `nami doctor --about`.
pub(crate) fn doctor_about(json: bool) -> Result<()> {
    let a = hardware::about(&hardware::sys_root());
    if json {
        println!("{a}");
    } else {
        print!("{}", hardware::about_text(&a));
    }
    Ok(())
}

/// `nami doctor --hardware`.
pub(crate) fn doctor_hardware(json: bool) -> Result<()> {
    let file = std::fs::canonicalize(system_config()).ok();
    for s in hardware::suggestions(&hardware::sys_root(), file.as_deref()) {
        if json {
            println!("{}", serde_json::to_value(&s)?);
        } else {
            println!("{} ({}): {}{}", s.model.as_deref().unwrap_or(&s.hardware), s.hardware, s.package, if s.listed { ", listed" } else { "" });
        }
    }
    Ok(())
}

/// `nami report`.
pub(crate) fn send_report(root: &std::path::Path, message: Option<String>, yes: bool, no_files: bool, show: bool, json: bool) -> Result<()> {
    let message = match message {
        Some(m) => m,
        None if show => String::new(),
        None => report::ask_message()?,
    };
    let r = report::gather(message, !no_files);
    if show {
        if json {
            println!("{}", serde_json::to_string(&r)?);
        } else {
            print!("{}", report::show(&r));
        }
        return Ok(());
    }
    if !yes {
        println!("\nThis is what would be sent:\n\n{}", report::show(&r));
        if !report::confirm()? {
            println!("Not sent.");
            return Ok(());
        }
    }
    let store = Store::open_direct_readonly(&root)?;
    let id = report::send(&store, &r)?;
    if json {
        println!("{}", serde_json::json!({ "id": id }));
    } else {
        println!("{}", style::out().line(&format!("✓ Sent as report {id} · mention that number if you write about it · thank you!")));
    }
    Ok(())
}

/// `nami self-update`, as root (or someone who isn't an admin), or `--check`.
pub(crate) fn self_update(root: &std::path::Path, check: bool, json: bool) -> Result<()> {
    let store = if profile::is_root() { Store::open_direct(&root, false)? } else { Store::open_direct_readonly(&root)? };
    if !check {
        return selfupdate::run(&store);
    }
    let c = selfupdate::check(&store)?;
    if json {
        println!("{}", serde_json::to_string(&c)?);
    } else if c.newer {
        println!("nami {} is out (you have {}): nami self-update", c.latest, c.current);
    } else {
        println!("nami {} is the newest version", c.current);
    }
    Ok(())
}

/// `nami doctor`.
pub(crate) fn doctor(store: &Store, profiles: &Profiles, json: bool) -> Result<()> {
    // --json: JSON lines alone (the devices' packages, then each check).
    // Else a card, ✓ fine, ⚠ worth a look (and what to do), ✗ wrong.
    let paint = style::out();
    let say = |l: &str| println!("{}", paint.line(&format!("  {l}")));
    if !json {
        println!("{}", paint.bold("This computer"));
        match store.nix() {
            Some(nix) => say(&format!("✓ Store · shared with Nix ({}) · its GC roots in /nix/var/nix/gcroots/tray", nix.bin().display())),
            None => say("✓ Store · nami's own (no Nix on this computer)"),
        }
        let about: std::collections::BTreeMap<String, String> = runtime::describe(store.root()).into_iter().collect();
        let runtime = profiles.current()?.map(|m| m.runtime).unwrap_or_default();
        let count_of = |k: &str| about.get(k).filter(|v| *v != "none").map_or(0, |v| v.split_whitespace().count());
        let (mesa, vulkan) = (count_of("mesa drivers"), count_of("vulkan ICDs"));
        let linked = about.get(runtime::OPENGL_DRIVER).is_some_and(|v| v != "missing");
        if linked && mesa > 0 {
            say(&format!("✓ Graphics · {} Mesa drivers, {} Vulkan drivers", mesa, vulkan));
        } else if runtime.is_empty() {
            say("✓ Graphics · none needed yet (no apps with windows)");
        } else {
            say(&format!("⚠ Graphics drivers aren't set up ({} is missing) · nami apply", runtime::OPENGL_DRIVER));
        }
        if let Some(v) = about.get("nvidia module").filter(|v| *v != "not loaded") {
            say(&format!("✓ NVIDIA driver {v}"));
        }
        match services::backend(store.root()) {
            services::Backend::Live => say("✓ Services · systemd starts them"),
            services::Backend::Offline => say(&format!("⚠ Services · set up, not started: {} is a scratch root", store.root().display())),
            services::Backend::Unsupported(why) => say(&format!("⚠ Services · {why}")),
        }
        if !runtime.is_empty() {
            let names: Vec<String> = runtime.iter().map(|e| tell::name_version(&e.name).0).collect();
            say(&format!("✓ For apps with windows · {}", names.join(", ")));
        }
    }
    if cfg!(target_os = "linux") {
        // system.nami, if there's one, to say what it lists.
        let file = std::fs::canonicalize(system_config()).ok();
        let found = hardware::suggestions(&hardware::sys_root(), file.as_deref());
        for s in &found {
            if json {
                println!("{}", serde_json::to_value(s)?);
            } else if !s.listed {
                let folder = file.as_deref().and_then(std::path::Path::parent).map_or("<computer's folder>".into(), |p| p.display().to_string());
                say(&format!("⚠ This computer has {} · the {} package is for it", s.hardware, s.package));
                println!("    {}{}{}", paint.accent(&format!("nami new {folder} --from {}", s.package)), paint.dim(", then list "), paint.dim(&format!("- {} in its system.nami", s.package)));
            }
        }
    }
    // (system.nami is the admins' alone: its packages' checks too.)
    if crate::nami_os::here() && std::fs::File::open(system_config()).is_err() {
        if !json {
            println!("\n{}", paint.dim("What this computer's packages set up is an administrator's to check (system.nami is theirs)."));
        }
    } else if crate::nami_os::here() {
        let file = system_config();
        if !json {
            println!("\n{} {}", paint.bold("What its packages set up"), paint.dim(&format!("· {}'s checks", file.display())));
        }
        let results = doctor::run(&file, |o| {
            if json {
                println!("{}", serde_json::json!({ "check": o.name, "ok": o.problem.is_none(), "saw": o.problem }));
            } else {
                match &o.problem {
                    None => println!("{}", paint.line(&format!("  ✓ {}", o.name))),
                    Some(saw) => println!("{}\n{}", paint.line(&format!("  ✗ {}", o.name)), paint.dim(saw.lines().map(|l| format!("      {l}\n")).collect::<String>().trim_end())),
                }
            }
        })?;
        let failed = results.iter().filter(|o| o.problem.is_some()).count();
        if failed > 0 {
            eprintln!("{}", style::err().line(&format!("✗ {failed} of {} checks failed · nami report tells nami's makers, with this", results.len())));
            std::process::exit(1);
        }
        if !json {
            println!("{}", paint.line(&format!("✓ All {} checks passed", results.len())));
        }
    }
    Ok(())
}
