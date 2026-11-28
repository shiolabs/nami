//! `nami config`: a nami file read and changed a value at a time (Settings' way in).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::cli::ConfigCommand;
use crate::cmd::files::{default_config, is_system_config, personal_config, system_config};
use crate::cmd::setup::{repo_note_set_seen, repo_system_file};
use crate::{config, config_edit, daemon, oldnames, profile, proposals, secrets, store, style, yaml_text};

fn op_path(op: &config_edit::Op) -> &[String] {
    match op {
        config_edit::Op::Set { path, .. } | config_edit::Op::Unset { path } | config_edit::Op::Add { path, .. } | config_edit::Op::Remove { path, .. } => path,
    }
}

/// Text edits of a config; no root, no daemon.
/// Whether a change to `file` goes through the daemon: it's the computer's
/// file, which only root writes, and this isn't root (an admin, whose
/// password the daemon asks for).
pub(crate) fn through_daemon(file: &std::path::Path) -> bool {
    if profile::is_root() || !is_system_config(file) || !file.exists() {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(file.as_os_str().as_encoded_bytes()) else { return false };
    // SAFETY: a valid C path.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) != 0 }
}

/// `nami config <args> --file <system.nami>`, run as root by the
/// daemon for an admin once they've given their password; `input` on its stdin.
fn config_as_admin(root: &std::path::Path, mut args: Vec<String>, input: String) -> Result<()> {
    let socket = store::daemon_socket(root);
    anyhow::ensure!(socket.exists(), "only root changes {} (nami's daemon isn't running here: sudo nami config ...)", system_config().display());
    args.extend(["--file".to_string(), system_config().display().to_string()]);
    let code = daemon::client::run_with(&socket, &args, std::path::Path::new("/"), Some(input))?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

pub(crate) fn config_command(root: &std::path::Path, command: ConfigCommand, file: Option<PathBuf>) -> Result<()> {
    if let ConfigCommand::Path { personal } = command {
        println!("{}", if personal { personal_config() } else { file.unwrap_or_else(default_config) }.display());
        return Ok(());
    }
    if let ConfigCommand::Get { path, json, text: whole } = command {
        // (The file `nami config set` would change for that path: env.EDITOR is yours.)
        let personal = path.as_deref().filter(|p| !p.is_empty()).and_then(|p| config_edit::parse_path(p).ok()).is_some_and(|keys| personal_path(&keys));
        let file = file.unwrap_or_else(if personal { personal_config } else { default_config });
        // The computer's, for someone who isn't an admin, once Settings is unlocked: read by
        // the daemon, while polkit keeps the admin's password given a moment ago.
        if through_daemon(&file) && std::fs::File::open(&file).is_err() {
            let mut args = vec!["config".to_string(), "get".to_string()];
            args.extend(path);
            if json {
                args.push("--json".into());
            }
            if whole {
                args.push("--text".into());
            }
            return config_as_admin(root, args, String::new());
        }
        let text = if file.as_os_str() == "-" {
            let mut t = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)?;
            t
        } else {
            // (tray's name for it, while it hasn't been renamed: `user.tray`.)
            std::fs::read_to_string(oldnames::find(&file)).unwrap_or_default()
        };
        if whole {
            print!("{text}");
            return Ok(());
        }
        let mut at: serde_yaml::Value = if text.trim().is_empty() { serde_yaml::Value::Mapping(Default::default()) } else { yaml_text::from_str(&text).with_context(|| format!("reading {}", file.display()))? };
        // (tray's `trayos:` is `namios:`.)
        oldnames::keys(&mut at);
        let keys = match path.as_deref() { Some(p) if !p.is_empty() => config_edit::parse_path(p)?, _ => Vec::new() };
        for key in keys {
            at = match (&at, key.parse::<usize>()) {
                (serde_yaml::Value::Sequence(s), Ok(i)) => s.get(i).cloned().unwrap_or_default(),
                _ => at.get(key.as_str()).cloned().unwrap_or_default(),
            };
        }
        if json {
            println!("{}", serde_json::to_string(&at)?);
        } else if !at.is_null() {
            print!("{}", style::out().lines(&serde_yaml::to_string(&at)?, style::Paint::yaml_line));
        } else if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            // (Nothing, for a script; in words, for someone reading.)
            let what = path.as_deref().filter(|p| !p.is_empty()).unwrap_or("anything");
            eprintln!("{}", style::err().dim(&format!("{} has no {what}", shown_file(&file))));
        }
        return Ok(());
    }
    if let ConfigCommand::Unlock { about } = command {
        // (Root, or a file that's theirs to write: nothing to ask.)
        let file = file.unwrap_or_else(default_config);
        if through_daemon(&file) {
            return config_as_admin(root, ["config".to_string(), "unlock".to_string()].into_iter().chain(about).collect(), String::new());
        }
        return Ok(());
    }
    if let ConfigCommand::Write { expect } = command {
        anyhow::ensure!(file.as_deref() != Some(std::path::Path::new("-")), "the text and the file can't both come from stdin");
        let file = file.unwrap_or_else(default_config);
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        if through_daemon(&file) {
            let mut args = vec!["config".to_string(), "write".to_string()];
            if let Some(e) = expect {
                args.extend(["--expect".to_string(), e]);
            }
            return config_as_admin(root, args, text);
        }
        let file = oldnames::for_writing(&file);
        let new = !file.exists();
        let before = std::fs::read_to_string(&file).unwrap_or_default();
        // (A proposal approved: only onto the file it was made against.)
        if let Some(e) = expect {
            anyhow::ensure!(proposals::digest(&before) == e, "{} changed since this was proposed: nothing was written", file.display());
        }
        std::fs::write(&file, &text)?;
        if new && profile::is_root() && is_system_config(&file) {
            profile::for_admins(&file)?;
        }
        if profile::is_root() && is_system_config(&file) {
            crate::nami_os::system_file_changed(&before, &text);
        }
        return Ok(());
    }
    // What the daemon is asked to run, for system.nami (below).
    let mut stdin_op = None;
    let forward: Vec<String> = match &command {
        ConfigCommand::Set { path, value } => vec!["set".into(), path.clone(), value.clone()],
        ConfigCommand::Unset { path } => vec!["unset".into(), path.clone()],
        ConfigCommand::Add { path, value } => vec!["add".into(), path.clone(), value.clone()],
        ConfigCommand::Remove { path, value } => vec!["remove".into(), path.clone(), value.clone()],
        ConfigCommand::Json { .. } => vec!["json".into(), "-".into()],
        _ => Vec::new(),
    };
    // As it reads there in the file: `env.V 3.10` is the text 3.10.
    let value = |path: &[String], item: bool, v: &str| -> serde_yaml::Value { yaml_text::at(path, item, v).unwrap_or_else(|| serde_yaml::Value::String(v.into())) };
    let op = match command {
        ConfigCommand::Set { path, value: v } => {
            let path = config_edit::parse_path(&path)?;
            config_edit::Op::Set { value: value(&path, false, &v), path }
        }
        ConfigCommand::Unset { path } => config_edit::Op::Unset { path: config_edit::parse_path(&path)? },
        ConfigCommand::Add { path, value: v } => {
            let path = config_edit::parse_path(&path)?;
            config_edit::Op::Add { value: value(&path, true, &v), path }
        }
        ConfigCommand::Remove { path, value: v } => {
            let path = config_edit::parse_path(&path)?;
            config_edit::Op::Remove { value: value(&path, true, &v), path }
        }
        ConfigCommand::Json { op } => {
            let op = if op == "-" {
                anyhow::ensure!(file.as_deref() != Some(std::path::Path::new("-")), "the edit and the file can't both come from stdin");
                let mut t = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut t)?;
                t
            } else {
                op
            };
            stdin_op = Some(op.clone());
            config_edit::Op::from_json(&op)?
        }
        ConfigCommand::Path { .. } | ConfigCommand::Get { .. } | ConfigCommand::Write { .. } | ConfigCommand::Unlock { .. } => unreachable!(),
    };
    if file.as_deref() == Some(std::path::Path::new("-")) {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        print!("{}", config_edit::apply(&text, &op)?);
        return Ok(());
    }
    let personal = personal_path(op_path(&op));
    let file = file.unwrap_or_else(if personal { personal_config } else { default_config });
    if through_daemon(&file) {
        let mut args = vec!["config".to_string()];
        args.extend(forward);
        return config_as_admin(root, args, stdin_op.unwrap_or_default());
    }
    // Read where it is, by tray's name too while it hasn't been renamed (`user.tray`): read by
    // its new name, there was nothing yet, and the rename below put the edit alone in its place.
    let text = std::fs::read_to_string(oldnames::find(&file)).unwrap_or_default();
    // (tray's `trayos:`, written as `namios:` when it's what changes.)
    let new = config_edit::apply(&oldnames::keys_in_text(&text, op_path(&op)), &op)?;
    if new != text {
        if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = oldnames::for_writing(&file);
        let made = !file.exists();
        std::fs::write(&file, &new)?;
        if made && profile::is_root() && is_system_config(&file) {
            profile::for_admins(&file)?;
        }
        // A secret in system.nami, sealed at once: the admin who gave it can't write the file to.
        if profile::is_root() && is_system_config(&file) && new.contains("!secret") {
            if let Err(e) = secrets::seal(std::slice::from_ref(&file)) {
                eprintln!("{}", style::err().line(&format!("warning: the secret couldn't be sealed: {e:#}")));
            }
        }
        // The time zone, at once: nothing else uses it after the first start.
        if profile::is_root() && is_system_config(&file) {
            crate::nami_os::system_file_changed(&text, &new);
        }
        println!("{}", style::out().line(&format!("{} · nami apply to use it", config_edit::said(&op, &shown_file(&file), true))));
        // system.nami from a config repo: the repo's copy too.
        if let Some(r) = repo_system_file().filter(|_| is_system_config(&file)) {
            // Another admin's checkout may not be this person's to write:
            // system.nami changed all the same, and `nami apply` takes it there.
            let edited = std::fs::read_to_string(&r).map_err(anyhow::Error::from).and_then(|theirs| config_edit::apply(&theirs, &op)).and_then(|theirs_new| {
                std::fs::write(&r, &theirs_new)?;
                Ok(theirs_new)
            });
            match edited {
                Ok(theirs_new) => {
                    if theirs_new == new {
                        // `nami apply` notes it too, when the note isn't this person's to write.
                        let _ = repo_note_set_seen(&new);
                    }
                    println!("{}", style::out().line(&format!("  Your config repo's copy too · {} · commit and push it there", r.display())));
                }
                Err(e) => eprintln!("{}", style::err().line(&format!("warning: {} not updated yet ({e}): `nami apply` takes the change there", r.display()))),
            }
        }
    } else {
        println!("{}", style::out().line(&config_edit::said(&op, &shown_file(&file), false)));
    }
    Ok(())
}

/// Whether a path in a nami file is a home setting (yours: user.nami), not the computer's.
fn personal_path(path: &[String]) -> bool {
    match path {
        [k, ..] if config::USER_KEYS.contains(&k.as_str()) || k == "settings" => true,
        [k, f, ..] => k == "files" && f.starts_with("~/"),
        _ => false,
    }
}

/// A nami file as a command says it: `user.nami`, `system.nami`, else its path.
fn shown_file(file: &std::path::Path) -> String {
    let own = [personal_config(), system_config()].iter().any(|f| f == file);
    match file.file_name() {
        Some(n) if own => n.to_string_lossy().into_owned(),
        _ => file.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A setting changed in `user.nami` while the file is still tray's `user.tray`: the file's
    /// renamed, and keeps all it had (it once kept only the setting, everything else gone).
    #[test]
    fn an_edit_by_the_new_name_keeps_the_old_files_text() {
        let d = std::env::temp_dir().join(format!("nami-config-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("user.tray"), "packages:\n  - geogebra\ntrayos:\n  power:\n    quiet:\n      when: hours\n      percent: 60\n").unwrap();
        let op = r#"{"set":{"path":["namios","dock","on"],"value":false}}"#.to_string();
        config_command(&d, ConfigCommand::Json { op }, Some(d.join("user.nami"))).unwrap();
        let text = std::fs::read_to_string(d.join("user.nami")).unwrap();
        assert!(text.contains("- geogebra") && text.contains("when: hours") && text.contains("percent: 60") && text.contains("on: false"), "{text}");
        assert_eq!(std::fs::read_link(d.join("user.tray")).unwrap(), PathBuf::from("user.nami"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
