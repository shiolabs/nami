//! `nami propose` and `nami proposals`: changes to system.nami asked for, for an
//! administrator to approve (`nami mcp` proposes through these).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::index::Index;
use crate::store::Store;
use crate::cli::ProposalsCommand;
use crate::cmd::files::{is_system_config, system_config};
use crate::cmd::apply::check_text;
use crate::cmd::config::through_daemon;
use crate::cmd::setup::repo_system_file;
use crate::cmd::daemon::daemon_program;
use crate::{config, daemon, file_diff, profile, proposals, store, style, tell};

/// How `nami propose` answers: at once, or once it's decided (`--wait`).
pub(crate) struct Waiting {
    pub(crate) wait: bool,
    pub(crate) timeout: u64,
    pub(crate) json: bool,
    pub(crate) quiet: bool,
}

/// system.nami as text, for a proposal; why not, in plain words.
fn system_text() -> Result<(PathBuf, String)> {
    let file = system_config();
    match std::fs::read_to_string(&file) {
        Ok(t) => Ok((file, t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => anyhow::bail!("this computer has no {}: there's nothing to propose a change to", file.display()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => anyhow::bail!("{} can be read by administrators only: ask one to make the change", file.display()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// `nami check` of a text as system.nami, with the catalog to check names by.
fn system_check(root: &std::path::Path) -> impl Fn(&str) -> Vec<config::Diagnostic> {
    let index = Store::open_direct_readonly(root).ok().and_then(|s| Index::load(&s).ok());
    let file = system_config();
    move |text: &str| check_text(&file, text, None, index.as_ref())
}

/// Whether there's a desktop to tell (a session bus).
fn has_session_bus() -> bool {
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() || std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("bus").exists())
}

pub(crate) fn propose_command(root: &std::path::Path, edit: &str, reason: &str, by: Option<&str>, w: Waiting) -> Result<()> {
    let edit = if edit == "-" {
        let mut t = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)?;
        t
    } else {
        edit.to_string()
    };
    let edits = proposals::edits_of(&edit)?;
    let (file, text) = system_text()?;
    let dir = proposals::dir();
    let r = proposals::create(&dir, &file, &text, edits, reason, by.unwrap_or("An app"), system_check(root))?;
    let id = r.proposal.id.clone();
    if !w.quiet && has_session_bus() {
        use std::os::unix::process::CommandExt;
        // (On its own: it waits for the notification's button, and this doesn't.)
        let _ = std::process::Command::new(daemon_program()).args(["proposals", "notify", &id]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0).spawn();
    }
    if w.wait {
        if !w.json {
            eprintln!("waiting for an administrator to approve proposal {id} (Settings shows it)");
        }
        let p = proposals::wait(&dir, &id, w.timeout)?;
        println!("{}", proposals::result_json(&p));
        return Ok(());
    }
    if w.json {
        println!("{}", serde_json::to_string(&r)?);
        return Ok(());
    }
    let paint = style::out();
    println!("{}", paint.line(&format!("proposal {id}: waiting for approval")));
    if let Some(d) = &r.diff {
        let mut out = String::new();
        file_diff::print(d, &mut out);
        print!("{}", paint.lines(&out, style::Paint::diff_line));
    }
    // (Short lines: an agent's terminal is often narrow.)
    println!("Review it in Settings, or:\n  nami proposals show {id}");
    Ok(())
}

fn print_review(r: &proposals::Review) {
    let paint = style::out();
    let p = &r.proposal;
    println!("{}", paint.bold(&format!("{} ({})", proposals::headline(&p.by), p.id)));
    println!("  {}", p.reason);
    println!("  {}", paint.dim(&format!("{} · {}", p.status.word(), tell::ago(p.created, tell::now()))));
    if let Some(m) = &p.message {
        println!("  {m}");
    }
    if p.status == proposals::Status::Pending && r.stale {
        println!("{}", paint.line("  ⚠ system.nami changed since it was proposed, so it can't be applied · ask for it again"));
    }
    if let Some(d) = &r.diff {
        let mut out = String::new();
        file_diff::print(d, &mut out);
        print!("{}", paint.lines(&out, style::Paint::diff_line));
        print!("{}", paint.lines(&d.raw, style::Paint::diff_line));
    }
    for d in &r.problems {
        let sev = match d.severity { config::Severity::Error => "error", config::Severity::Warning => "warning", config::Severity::Note => "note" };
        println!("  {}: {}", paint.severity(sev), paint.message(&d.message));
    }
}

pub(crate) fn proposals_command(root: &std::path::Path, command: ProposalsCommand) -> Result<()> {
    let dir = proposals::dir();
    // The file now, to look at each against (none: nothing's applicable, all are shown as stale).
    let current = || system_text().map(|(_, t)| t).unwrap_or_default();
    match command {
        ProposalsCommand::List { all, json } => {
            let list: Vec<proposals::Proposal> = proposals::list(&dir).into_iter().filter(|p| all || p.status == proposals::Status::Pending).collect();
            let text = current();
            let check = system_check(root);
            let reviews: Vec<proposals::Review> = list.into_iter().map(|p| proposals::review(p, &text, &check)).collect();
            if json {
                println!("{}", serde_json::to_string(&reviews)?);
            } else if reviews.is_empty() {
                println!("No proposals{}.", if all { "" } else { " waiting" });
            } else {
                for r in &reviews {
                    let p = &r.proposal;
                    let what = if r.stale && p.status == proposals::Status::Pending { "stale: system.nami changed since".to_string() } else { r.summary.clone() };
                    // The id dim (what `nami proposals show` takes), its state by colour: waiting amber, done green, not red.
                    let paint = style::out();
                    let word = format!("{:<8}", p.status.word());
                    let word = match p.status {
                        proposals::Status::Pending => paint.changed(&word),
                        proposals::Status::Applied => paint.added(&word),
                        _ => paint.removed(&word),
                    };
                    println!("{}  {word}  {}  {}  {}", paint.dim(&p.id), paint.bold(&p.by), what, paint.dim(&format!("({})", p.reason)));
                }
            }
        }
        ProposalsCommand::Show { id, json } => {
            let r = proposals::review(proposals::load(&dir, &id)?, &current(), system_check(root));
            if json {
                println!("{}", serde_json::to_string(&r)?);
            } else {
                print_review(&r);
            }
        }
        ProposalsCommand::Reject { id } => {
            let p = proposals::decide(&dir, &id, proposals::Status::Rejected, Some("Not allowed: system.nami wasn't changed.".into()))?;
            eprintln!("{}", style::err().line(&format!("✓ Proposal {} discarded · {} is told it wasn't allowed", p.id, p.by)));
        }
        ProposalsCommand::Apply { id, expect } => return apply_proposal(root, &dir, &id, expect.as_deref()),
        ProposalsCommand::Notify { id } => notify_proposal(&dir, &id),
    }
    Ok(())
}

/// Approve proposal `id`: an administrator's password (the daemon asks polkit, saying what
/// changes), then system.nami as the proposal makes it, then applied.
fn apply_proposal(root: &std::path::Path, dir: &std::path::Path, id: &str, expect: Option<&str>) -> Result<()> {
    let p = proposals::load(dir, id)?;
    let (file, text) = system_text()?;
    anyhow::ensure!(is_system_config(std::path::Path::new(&p.file)), "proposal {id} is for {}, not this computer's file", p.file);
    let check = system_check(root);
    let after = proposals::ready(dir, &p, expect, &text, &check)?;
    let about = proposals::review(p.clone(), &text, &check).diff.map(|d| proposals::asked(&p.by, &d)).unwrap_or_else(|| format!("{} asked to change this computer's settings.", p.by));
    if through_daemon(&file) {
        let socket = store::daemon_socket(root);
        anyhow::ensure!(socket.exists(), "only root changes {} (nami's daemon isn't running here)", file.display());
        let sys = file.display().to_string();
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        // The password first, saying what changes; the write then goes on the one just given.
        let code = daemon::client::run_with(&socket, &args(&["config", "unlock", &about, "--file", &sys]), std::path::Path::new("/"), None)?;
        anyhow::ensure!(code == 0, "nothing was changed: an administrator's password is needed to apply it");
        let code = daemon::client::run_with(&socket, &args(&["config", "write", "--expect", &p.base, "--file", &sys]), std::path::Path::new("/"), Some(after.clone()))?;
        if code != 0 {
            // (Changed in the moment between: stale. Else it stays to try again.)
            if std::fs::read_to_string(&file).map(|t| proposals::digest(&t)).ok().as_deref() != Some(p.base.as_str()) {
                let _ = proposals::decide(dir, id, proposals::Status::Stale, Some(format!("system.nami changed since {} asked: nothing was changed. Ask for the change again.", p.by)));
            }
            anyhow::bail!("system.nami wasn't changed");
        }
    } else {
        let now = std::fs::read_to_string(&file)?;
        anyhow::ensure!(proposals::digest(&now) == p.base, "system.nami changed since {} asked: nothing was changed", p.by);
        std::fs::write(&file, &after)?;
        if profile::is_root() {
            crate::nami_os::system_file_changed(&now, &after);
        }
    }
    eprintln!("updated {}", file.display());
    // system.nami from a config repo: the repo's copy too, as `nami config` does.
    if let Some(r) = repo_system_file() {
        match std::fs::read_to_string(&r).map_err(anyhow::Error::from).and_then(|theirs| proposals::edited(&theirs, &p.edits)).and_then(|theirs_new| Ok(std::fs::write(&r, &theirs_new)?)) {
            Ok(()) => eprintln!("updated {} (your config repo's: commit and push there)", r.display()),
            Err(e) => eprintln!("{} not updated yet ({e}): `nami apply` takes the change there", r.display()),
        }
    }
    // Then in place, as `sudo nami apply` does.
    let code = if through_daemon(&file) {
        daemon::client::run_with(&store::daemon_socket(root), &["apply".to_string(), "--system".to_string()], std::path::Path::new("/"), None)?
    } else {
        std::process::Command::new(daemon_program()).arg("apply").arg(&file).status().map(|s| s.code().unwrap_or(1)).unwrap_or(1)
    };
    if code == 0 {
        proposals::decide(dir, id, proposals::Status::Applied, Some("system.nami was changed and applied.".into()))?;
        eprintln!("applied proposal {id}");
        Ok(())
    } else {
        proposals::decide(dir, id, proposals::Status::Failed, Some("system.nami was changed, but applying it failed: `sudo nami apply` says why.".into()))?;
        std::process::exit(code);
    }
}

/// The notification of a proposal: "Claude wants to change your computer", its reason, and
/// Review, which opens Settings at it. (Waits for the button: started on its own.)
fn notify_proposal(dir: &std::path::Path, id: &str) {
    let Ok(p) = proposals::load(dir, id) else { return };
    let mut cmd = std::process::Command::new("notify-send");
    cmd.args(["--app-name=Settings", "--icon=nami-settings", "--hint=string:desktop-entry:nami-gui", "--action=default=Review", "--action=review=Review", "--wait"]);
    cmd.arg(proposals::headline(&p.by)).arg(&p.reason);
    let Ok(out) = cmd.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output() else { return };
    if matches!(String::from_utf8_lossy(&out.stdout).trim(), "review" | "default") {
        use std::os::unix::process::CommandExt;
        let _ = std::process::Command::new("nami-gui").args(["--page", "requests"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0).spawn();
    }
}
