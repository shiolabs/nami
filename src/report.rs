//! `nami report`: tell nami's makers something's wrong. What happened (in
//! your words), nami's version, the system, `nami doctor`, the last steps
//! (`nami history`) and system.nami and yours, with every secret's
//! value left out. It's all shown before it goes, and sent to the site nami
//! came from (`<site>/report`), which answers with the report's number.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::store::Store;

#[derive(Serialize)]
pub struct Report {
    pub message: String,
    pub version: String,
    pub system: String,
    pub doctor: String,
    pub history: String,
    /// Each file by where it is, its secrets left out.
    pub files: Vec<(String, String)>,
}

/// A `!secret` value, in a line or as a block after it: `<secret>` instead.
pub fn without_secrets(text: &str) -> String {
    let mut out = Vec::new();
    let mut block: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if let Some(i) = block {
            if line.trim().is_empty() || indent > i {
                continue;
            }
            block = None;
        }
        match line.find("!secret") {
            Some(at) => {
                let rest = line[at + "!secret".len()..].trim();
                if rest.starts_with('|') || rest.starts_with('>') {
                    block = Some(indent);
                }
                out.push(format!("{}<secret>", &line[..at]));
            }
            None => out.push(line.to_string()),
        }
    }
    out.join("\n")
}

/// What `nami <args>` prints (its errors too), run as you.
fn nami(args: &[&str]) -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| "nami".into());
    match std::process::Command::new(exe).args(args).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s.trim_end().to_string()
        }
        Err(e) => format!("couldn't run nami {}: {e}", args.join(" ")),
    }
}

fn system() -> String {
    let root = std::path::PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into()));
    let release = std::fs::read_to_string(root.join("etc/os-release")).unwrap_or_default();
    let pretty = release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string());
    format!("{} ({})", pretty.unwrap_or_else(|| "unknown Linux".into()), std::env::consts::ARCH)
}

/// Everything but what happened, which `message` is.
pub fn gather(message: String, files: bool) -> Report {
    let mut found = Vec::new();
    if files {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        for path in [crate::config::system_file(), crate::config::user_file()] {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let shown = path.strip_prefix(&home).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| path.display().to_string());
                found.push((shown, without_secrets(&text)));
            }
        }
    }
    // Yours, and the computer's.
    let last = |h: String| h.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    let history = format!("{}\n\nThe system's (nami history --system):\n{}", last(nami(&["history"])), last(nami(&["history", "--system"])));
    Report { message, version: env!("CARGO_PKG_VERSION").into(), system: system(), doctor: nami(&["doctor"]), history, files: found }
}

/// The report as it's shown before sending.
pub fn show(r: &Report) -> String {
    let mut s = format!("What happened:\n{}\n\nnami {} on {}\n\nnami doctor:\n{}\n\nThe last changes (nami history):\n{}\n", r.message.trim_end(), r.version, r.system, r.doctor, r.history);
    for (path, text) in &r.files {
        s.push_str(&format!("\n{path}:\n{}\n", text.trim_end()));
    }
    s
}

/// What happened, typed at the terminal: lines until an empty one.
pub fn ask_message() -> Result<String> {
    if !crate::errors::may_ask() {
        return Err(crate::errors::needs_input("nami report asks what happened", "--message \"...\""));
    }
    println!("What happened? What you did, what you expected, what happened instead. (End with an empty line.)");
    let mut lines = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            break;
        }
        lines.push(line);
    }
    let message = lines.join("\n");
    if message.trim().is_empty() {
        bail!("nothing said, so nothing sent");
    }
    Ok(message)
}

pub fn confirm() -> Result<bool> {
    if !crate::errors::may_ask() {
        return Err(crate::errors::needs_input("nami report asks before sending", "--yes to send it"));
    }
    print!("Send it? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// Send it; the report's number back.
pub fn send(store: &Store, r: &Report) -> Result<String> {
    let url = format!("{}/report", crate::selfupdate::base(store)?);
    let body = serde_json::to_string(r)?;
    let reply = match store.http().post(&url).set("content-type", "application/json").send_string(&body) {
        Ok(reply) => reply.into_string().context("reading the answer")?,
        Err(ureq::Error::Status(code, reply)) => bail!("{url} said {code}: {}", reply.into_string().unwrap_or_default().trim()),
        Err(e) => return Err(e).with_context(|| format!("sending to {url}")),
    };
    let v: serde_json::Value = serde_json::from_str(&reply).with_context(|| format!("{url} answered {reply:?}"))?;
    v.get("id").and_then(|i| i.as_str()).map(String::from).with_context(|| format!("{url} answered {reply:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_left_out() {
        let text = "env:\n  KEY: !secret \"YWdl\"\n  OTHER: 1\nfiles:\n  a: !secret |\n    line one\n    line two\n  b: 2\n";
        assert_eq!(without_secrets(text), "env:\n  KEY: <secret>\n  OTHER: 1\nfiles:\n  a: <secret>\n  b: 2");
    }
}
