//! `nami home`: the settings files apps change, kept, undone or adopted.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::profile::Profiles;
use crate::store::Store;
use crate::cli::HomeCommand;
use crate::cmd::files::personal_config;
use crate::cmd::apply::edit_and_apply;
use crate::{config_edit, drift, home, style, yaml_text};

pub(crate) fn home_command(store: &Store, profiles: &Profiles, command: HomeCommand, config: Option<PathBuf>) -> Result<()> {
    let home_dir = PathBuf::from(std::env::var_os("HOME").context("HOME isn't set")?);
    let link_base = profiles.home_link_base();
    let drifted = home::drift(&home_dir, &link_base)?;
    // `~/x`, `/home/me/x` or `x`: relative to home.
    let rel_of = |f: &str| -> String {
        let p = std::path::Path::new(f);
        p.strip_prefix(&home_dir).map(|r| r.display().to_string()).unwrap_or_else(|_| f.trim_start_matches("~/").to_string())
    };
    let pick = |files: &[String]| -> Vec<&home::Drift> {
        drifted.iter().filter(|d| files.is_empty() || files.iter().any(|f| rel_of(f) == d.file)).collect()
    };
    match command {
        HomeCommand::Diff { json } => {
            if json {
                println!("{}", serde_json::to_string(&drifted)?);
            } else if drifted.is_empty() {
                println!("{}", style::out().line("✓ No changes: every editable settings file says what your spec says"));
            } else {
                // Each file bold, what changed in it under it; then what to do.
                let paint = style::out();
                for d in &drifted {
                    println!("{}", paint.bold(&format!("~/{}", d.file)));
                    if d.whole {
                        println!("  {}", paint.changed("changed (plain text: `nami home keep` takes the whole file)"));
                    }
                    for c in &d.changes {
                        println!("  {} {}", paint.changed("~"), drift::describe(c));
                    }
                }
                println!("\n{}{}{}{}", paint.dim("Keep them in your spec: "), paint.accent("nami home keep"), paint.dim(" · Undo them: "), paint.accent("nami home undo <file>"));
            }
        }
        HomeCommand::Undo { files } => {
            anyhow::ensure!(!files.is_empty(), "name the file(s) to undo, e.g. nami home undo ~/.config/Code/User/settings.json");
            for f in &files {
                let rel = rel_of(f);
                let rel = rel.as_str();
                home::undo(&home_dir, &link_base, rel)?;
                println!("{}", style::out().line(&format!("✓ ~/{rel} is back to what your spec says")));
            }
        }
        HomeCommand::Adopt { files } => {
            anyhow::ensure!(!files.is_empty(), "name the file(s), e.g. nami home adopt ~/.config/Code/User/settings.json");
            let file = config.or(profiles.managed_config()?).unwrap_or_else(personal_config);
            let mut ops = Vec::new();
            for f in &files {
                let rel = rel_of(f);
                home::check_rel(&rel)?;
                let text = std::fs::read_to_string(home_dir.join(&rel)).with_context(|| format!("reading ~/{rel}"))?;
                let content = match drift::parse(&rel, &text) {
                    Ok(Some(v)) => v,
                    Ok(None) => serde_yaml::Value::String(text),
                    Err(e) => anyhow::bail!("~/{rel} can't be read as settings: {e}"),
                };
                let mut entry = serde_yaml::Mapping::new();
                entry.insert("mutable".into(), true.into());
                entry.insert("content".into(), content);
                ops.push(config_edit::Op::Set { path: vec!["files".into(), format!("~/{rel}")], value: serde_yaml::Value::Mapping(entry) });
                println!("{}", style::out().line(&format!("✓ ~/{rel} is now in your spec")));
            }
            if !file.exists() {
                std::fs::create_dir_all(file.parent().unwrap_or(std::path::Path::new(".")))?;
                std::fs::write(&file, "")?;
            }
            edit_and_apply(store, profiles, &file, &ops, false)?;
        }
        HomeCommand::Keep { files } => {
            let todo = pick(&files);
            if todo.is_empty() {
                println!("{}", style::out().line("✓ Nothing to keep: no app changed its settings"));
                return Ok(());
            }
            let file = config.or(profiles.managed_config()?).unwrap_or_else(personal_config);
            let spec: serde_yaml::Value = yaml_text::from_str(&std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?)?;
            let mut ops = Vec::new();
            for d in todo {
                let (at, entry) = drift::spec_entry(&spec, &d.file)
                    .with_context(|| format!("{} doesn't set ~/{} (in files:, or under a package)", file.display(), d.file))?;
                let wrapped = entry.as_mapping().is_some_and(|m| m.contains_key("content"));
                if d.whole {
                    let text = std::fs::read_to_string(home_dir.join(&d.file))?;
                    ops.push(config_edit::Op::Set { path: drift::spec_path(&at, wrapped, &[]), value: serde_yaml::Value::String(text) });
                }
                for c in &d.changes {
                    let path = drift::spec_path(&at, wrapped, &c.path);
                    ops.push(match &c.to {
                        None => config_edit::Op::Unset { path },
                        Some(v) => config_edit::Op::Set { path, value: drift::typed_like(v, c.from.as_ref()) },
                    });
                    println!("~/{}: {}", d.file, drift::describe(c));
                }
            }
            edit_and_apply(store, profiles, &file, &ops, false)?;
        }
    }
    Ok(())
}
