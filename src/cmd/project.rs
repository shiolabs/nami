//! Projects: `nami new`, `shell`, `run`, `env`, `up`, `down`, `logs`, `project`, and
//! the shell hook (`hook`, `hook-env`).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::index::Index;
use crate::profile::Profiles;
use crate::store::Store;
use crate::cmd::files::system_config;
use crate::cmd::setup::share_config_with_admins;
use crate::{daemon, new, profile, project, store, style, tell, up};

/// `nami hook-env`.
pub(crate) fn hook_env(root: &std::path::Path, fish: bool) -> Result<()> {
    // Silent and quick when nothing changed: the store is only opened to load a project.
    let out = project::hook_env(fish, |file| {
        let store = Store::open(&root)?;
        let env = project::prepare(&store, &Profiles::personal(&store)?, file, false, false)?;
        env.all_vars()
    })?;
    print!("{out}");
    Ok(())
}

/// `nami new`.
pub(crate) fn new(root: &std::path::Path, folder: Option<PathBuf>, from: Option<String>, force: bool, json: bool) -> Result<()> {
    match from {
        None if json => println!("{}", new::list_json()),
        None => new::list(),
        Some(name) => {
            let dir = folder.unwrap_or_else(|| PathBuf::from("."));
            // The computer's folder, not theirs to write (someone who isn't an admin, with
            // Settings unlocked): copied in by the daemon, as root, with an admin's password.
            let shared = new::packages_next_to(&dir);
            let computers = |d: &std::path::Path| d.canonicalize().ok().is_some_and(|d| system_config().parent().and_then(|p| p.canonicalize().ok()) == Some(d));
            let theirs = std::ffi::CString::new(shared.as_os_str().as_encoded_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0);
            if !profile::is_root() && !force && !theirs && new::is_package(&name) && computers(&dir) && store::daemon_socket(root).exists() {
                let args = vec!["new".to_string(), dir.display().to_string(), "--from".into(), name];
                let code = daemon::client::run(&store::daemon_socket(root), &args, std::path::Path::new("/"))?;
                if code != 0 {
                    std::process::exit(code);
                }
                return Ok(());
            }
            new::new(&dir, &name, force)?;
            // (Root's copy, for someone unlocked: the admins' to change, as theirs is.)
            if profile::is_root() && computers(&dir) {
                share_config_with_admins()?;
            }
            // A project: listed as yours straight away.
            if let (Some(_), Ok(dir)) = (project::file_in(&dir), dir.canonicalize()) {
                if let Ok(store) = Store::open(&root) {
                    Profiles::personal(&store).and_then(|p| p.note_project(&dir)).unwrap_or_else(|e| eprintln!("warning: couldn't list the project as yours: {e:#}"));
                }
            }
        }
    }
    Ok(())
}

/// `nami project list`.
pub(crate) fn list_projects(root: &std::path::Path, json: bool) -> Result<()> {
    let store = Store::open(&root)?;
    let profiles = Profiles::personal(&store)?; // projects are yours
    let index = Index::load(&store).ok();
    let projects: Vec<project::Summary> = profiles.projects().iter().filter_map(|d| project::summary(d, index.as_ref()).ok()).collect();
    if json {
        println!("{}", serde_json::to_string(&projects)?);
    } else if projects.is_empty() {
        println!("no projects yet: `nami new <folder> --from <starter>` (`nami new` lists them), then `nami shell` in it");
    } else {
        // Each one's name bold, where it is and how many tools dim.
        let paint = style::out();
        let width = projects.iter().map(|p| p.name.chars().count()).max().unwrap_or(0);
        for p in projects {
            println!("{}{}  {}", paint.bold(&p.name), " ".repeat(width - p.name.chars().count()), paint.dim(&format!("{} · {}", p.dir, tell::count(p.packages.len(), "tool", "tools"))));
        }
    }
    Ok(())
}

/// `nami project tasks`.
pub(crate) fn tasks(json: bool) -> Result<()> {
    let file = project::find(&std::env::current_dir()?).context("no project.nami here or above")?;
    let p = project::read(&file)?;
    if json {
        let tasks: Vec<serde_json::Value> = p.tasks.iter().map(|(name, t)| serde_json::json!({ "name": name, "run": t.run, "description": t.description, "depends": t.depends })).collect();
        println!("{}", serde_json::Value::Array(tasks));
        return Ok(());
    }
    if p.tasks.is_empty() {
        println!("no tasks in {}; add some:\n\n  tasks:\n    test: npm test\n", file.display());
    }
    // Each task's name bold (`nami run <name>`), what it runs after it.
    let paint = style::out();
    let width = p.tasks.keys().map(|n| n.chars().count()).max().unwrap_or(0).max(12);
    for (name, task) in &p.tasks {
        let first = if task.depends.is_empty() { String::new() } else { paint.dim(&format!(" (first: {})", task.depends.join(", "))) };
        println!("{}{} {}{first}", paint.bold(name), " ".repeat(width - name.chars().count()), task.shown());
    }
    Ok(())
}

/// `nami project services`.
pub(crate) fn services(json: bool) -> Result<()> {
    let file = project::locate_local(None)?;
    let dir = std::fs::canonicalize(file.parent().unwrap_or(std::path::Path::new(".")))?;
    let services = up::resolve(&project::read(&file)?.services, &dir)?;
    let status = up::status(&dir, &services);
    if json {
        println!("{}", serde_json::to_string(&status)?);
        return Ok(());
    }
    if status.is_empty() {
        println!("no services in {}; add some:\n\n  services:\n    - ./services/postgres.nami   (nami new . --from postgres writes it)\n", file.display());
    }
    let ports: Vec<String> = status.iter().map(|s| if s.ports.is_empty() { String::new() } else { format!("port {}", s.ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")) }).collect();
    let width = status.iter().map(|s| s.name.len()).max().unwrap_or(0);
    let port_width = ports.iter().map(String::len).max().unwrap_or(0);
    for (s, ports) in status.iter().zip(&ports) {
        let restarts = match s.restarts {
            0 => String::new(),
            1 => " (restarted once)".into(),
            n => format!(" (restarted {n} times)"),
        };
        println!("{:width$}  {:10} {ports:port_width$}  {}{restarts}", s.name, s.state, s.about.as_deref().unwrap_or(""));
    }
    Ok(())
}

/// `nami up`.
pub(crate) fn up(root: &std::path::Path, names: Vec<String>, detach: bool, p: Option<String>, locked: bool) -> Result<()> {
    let file = project::locate_local(p.as_deref())?;
    let env = {
        let store = Store::open(&root)?;
        project::prepare(&store, &Profiles::personal(&store)?, &file, locked, false)?
    };
    up::up(&env.dir, &env.services, &env.vars, &names, detach)
}

/// `nami down`.
pub(crate) fn down(names: Vec<String>, p: Option<String>) -> Result<()> {
    let file = project::locate_local(p.as_deref())?;
    let dir = std::fs::canonicalize(file.parent().unwrap_or(std::path::Path::new(".")))?;
    // A broken project.nami still stops what runs.
    let services = project::read(&file).and_then(|pr| up::resolve(&pr.services, &dir)).unwrap_or_default();
    up::down(&dir, &services, &names)
}

/// `nami logs`.
pub(crate) fn logs(name: Option<String>, p: Option<String>) -> Result<()> {
    let file = project::locate_local(p.as_deref())?;
    let dir = std::fs::canonicalize(file.parent().unwrap_or(std::path::Path::new(".")))?;
    up::logs(&dir, &up::resolve(&project::read(&file)?.services, &dir)?, name.as_deref())
}

/// `nami shell`.
pub(crate) fn shell(store: &Store, dir: Option<String>, p: Option<String>, locked: bool, upgrade: bool, command: Option<String>) -> Result<()> {
    let env = project::prepare(&store, &Profiles::personal(&store)?, &project::locate(&store, dir.or(p).as_deref())?, locked, upgrade)?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    // `-c`: one command in it, as `sh -c` runs it, its exit code nami's.
    if let Some(command) = command {
        let status = match &env.rc {
            Some(rc) => std::process::Command::new(project::bash_for(rc)).arg("-c").arg(format!("source \"$NAMI_RC\" >/dev/null 2>&1; {command}")).env("NAMI_RC", rc).envs(env.secrets.iter().cloned()).status().context("starting bash")?,
            None => std::process::Command::new(&shell).arg("-c").arg(&command).envs(env.vars.iter().cloned()).status().with_context(|| format!("starting {shell}"))?,
        };
        std::process::exit(status.code().unwrap_or(1));
    }
    // `nami shell · myapp · nodejs 22.1.0, ripgrep 15.2.0 · exit to leave`.
    let p = style::err();
    let names: Vec<String> = env.entries.iter().map(|e| tell::shown(&e.name)).collect();
    let project = env.dir.file_name().map_or_else(|| env.dir.display().to_string(), |n| n.to_string_lossy().into_owned());
    let tools = if names.is_empty() { p.dim("no tools") } else { names.join(", ") };
    eprintln!("{}{}{}{}{tools}{}{}{}", p.bold("nami shell"), p.dim(" · "), p.bold(&project), p.dim(" · "), p.dim(" · "), p.accent("exit"), p.dim(" to leave"));
    if !env.services.is_empty() && env.services.iter().all(|s| up::running(&env.dir, &s.name).is_none()) {
        let names: Vec<&str> = env.services.iter().map(|s| s.name.as_str()).collect();
        eprintln!("{}{}{}", p.dim(&format!("Services {}: not running · ", names.join(", "))), p.accent("nami up -d"), p.dim(" starts them"));
    }
    let status = match &env.rc {
        // develop: nixpkgs' setup is bash, so the shell is bash (like `nix develop`).
        Some(rc) => std::process::Command::new(project::bash_for(rc)).arg("--rcfile").arg(rc).arg("-i").envs(env.secrets.iter().cloned()).status().context("starting bash")?,
        None => std::process::Command::new(&shell).current_dir(std::env::current_dir()?).envs(env.vars.iter().cloned()).status().with_context(|| format!("starting {shell}"))?,
    };
    std::process::exit(status.code().unwrap_or(1));
}

/// `nami run`.
pub(crate) fn run(store: &Store, p: Option<String>, locked: bool, command: Vec<String>) -> Result<()> {
    let file = project::locate(&store, p.as_deref())?;
    let env = project::prepare(&store, &Profiles::personal(&store)?, &file, locked, false)?;
    // A task: its command line, with any extra arguments after it.
    let tasks = project::read(&file)?.tasks;
    if tasks.contains_key(&command[0]) {
        // What it depends on first, each once; the task itself gets
        // the extra arguments.
        let order = project::task_order(&tasks, &command[0])?;
        let args = &command[1..];
        for (i, name) in order.iter().enumerate() {
            let last = i + 1 == order.len();
            let Some(run) = &tasks[name].run else { continue };
            if !last {
                eprintln!("nami: {name}: {run}");
            }
            let script = format!("{run} \"$@\"");
            let args: &[String] = if last { args } else { &[] };
            let status = match &env.rc {
                Some(rc) => std::process::Command::new(project::bash_for(rc)).arg("-c").arg(format!("source \"$NAMI_RC\" >/dev/null 2>&1; {script}")).arg(name).args(args).env("NAMI_RC", rc).envs(env.secrets.iter().cloned()).status(),
                None => std::process::Command::new("sh").arg("-c").arg(&script).arg(name).args(args).envs(env.vars.iter().cloned()).status(),
            }
            .with_context(|| format!("running the task {name}"))?;
            if !status.success() || last {
                if !status.success() && !last {
                    eprintln!("nami: {name} failed, so {} didn't run", command[0]);
                }
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        std::process::exit(0);
    }
    anyhow::ensure!(!command.is_empty(), "nothing to run");
    if let Some(rc) = &env.rc {
        let status = std::process::Command::new(project::bash_for(rc))
            .arg("-c")
            .arg("source \"$0\" >/dev/null 2>&1; exec \"$@\"")

            .arg(rc)
            .args(&command)
            .envs(env.secrets.iter().cloned())
            .status()
            .context("starting bash")?;
        std::process::exit(status.code().unwrap_or(1));
    }
    let path = env.vars.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).unwrap_or_default();
    // Find the program on the project's PATH, not ours.
    let program = if command[0].contains('/') {
        PathBuf::from(&command[0])
    } else {
        path.split(':').map(|d| std::path::Path::new(d).join(&command[0])).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(&command[0]))
    };
    let status = std::process::Command::new(&program).args(&command[1..]).envs(env.vars.iter().cloned()).status().with_context(|| format!("running {}", command[0]))?;
    std::process::exit(status.code().unwrap_or(1));
}

/// `nami env`.
pub(crate) fn env(store: &Store, p: Option<String>, locked: bool, fish: bool) -> Result<()> {
    let env = project::prepare(&store, &Profiles::personal(&store)?, &project::locate(&store, p.as_deref())?, locked, false)?;
    print!("{}", project::exports(&env.all_vars()?, fish));
    Ok(())
}
