//! `nami mcp`: nami's tools for an AI agent, over the Model Context Protocol on stdio
//! (JSON-RPC, a message a line). No network: the agent's app starts it (`claude mcp add nami
//! -- nami mcp`) and talks to it on its stdin and stdout.
//!
//! Each tool runs nami itself, as the person, and gives back what it prints (JSON where nami
//! has it). What an agent may change is the person's own: their apps and ~/user.nami, applied
//! and undone. system.nami isn't: `propose` asks an administrator, who approves it in
//! Settings with their password (proposals.rs). A tool that would reach system.nami
//! (an app that sets up the computer, a removal from everyone's apps) says to propose instead,
//! so no password box ever comes up from here by surprise.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::Result;
use serde_json::{json, Map, Value};

/// The protocol versions this speaks, newest first.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// How long `propose` waits for the decision when asked to wait and not told how long.
const PROPOSE_WAIT: u64 = 300;

const INSTRUCTIONS: &str = "nami sets up this computer from text files. Your own setup is ~/user.nami (apps, shell, dotfiles, desktop settings): change it with install, remove or config, then apply; undo goes back. system.nami, /etc/nami/system.nami, is the administrator's: you can't change it, but propose asks them, with your reason, and they approve or decline it in Settings. Use get, check, diff and history to look before you change anything.";

#[derive(Default)]
struct Session {
    /// Who's asking, as the person reads it ("Claude"), from the client's name.
    by: Option<String>,
}

pub fn serve() -> Result<()> {
    let mut session = Session::default();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
            // (A batch, as 2025-03-26 has them: each answered, in one.)
            Ok(Value::Array(list)) => {
                let replies: Vec<Value> = list.into_iter().filter_map(|m| handle(&mut session, m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(m) => handle(&mut session, m),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(())
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// One message: its reply, or none for a notification.
fn handle(session: &mut Session, msg: Value) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    let Some(id) = msg.get("id").cloned() else {
        // A notification (initialized, cancelled): nothing to say back.
        return None;
    };
    if method.is_empty() {
        // (A reply to something this never asks.)
        return None;
    }
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method.as_str() {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
            let client = &params["clientInfo"];
            if let Some(name) = client.get("name").and_then(Value::as_str) {
                session.by = Some(crate::proposals::client_name(name, client.get("title").and_then(Value::as_str)));
            }
            ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "nami", "title": "nami", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            if !tools().iter().any(|t| t["name"] == name) {
                return Some(error(id, -32602, &format!("no tool named {name:?}")));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            ok(id, call(session, name, &args))
        }
        _ => error(id, -32601, &format!("method not found: {method}")),
    })
}

/// A tool's description: its name, title, what it does, its input's JSON Schema and hints.
fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str], read_only: bool, destructive: bool) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
        "annotations": { "title": title, "readOnlyHint": read_only, "destructiveHint": destructive, "idempotentHint": read_only, "openWorldHint": false },
    })
}

fn edit_schema() -> Value {
    json!({
        "description": "One edit, or a list of edits, as `nami config json` takes them: {\"set\": {\"path\": [\"env\", \"EDITOR\"], \"value\": \"nvim\"}}, {\"unset\": {\"path\": [...]}}, {\"add\": {\"path\": [\"packages\"], \"value\": \"ripgrep\"}}, {\"remove\": {\"path\": [\"packages\"], \"value\": \"ripgrep\"}}. A path is the keys from the top of the file; `nami options` lists them.",
        "anyOf": [{ "type": "object" }, { "type": "array", "items": { "type": "object" }, "minItems": 1 }],
    })
}

fn which_file() -> Value {
    json!({ "type": "string", "enum": ["user", "computer"], "default": "user", "description": "user: your ~/user.nami. computer: the computer's /etc/nami/system.nami (an administrator's)." })
}

fn packages_schema(what: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 20, "description": what })
}

/// The catalog, as search's description says it: what this kind of computer can install from it
/// (November 2026: 146,202 on x86_64 Linux, 87,731 on ARM Linux, 73,502 on an Apple silicon Mac,
/// 66,872 on an Intel one).
const CATALOG_SAID: &str = if crate::index::MAC {
    if cfg!(target_arch = "aarch64") { "over 70,000 ready-built for this Mac, from nixpkgs" } else { "over 60,000 ready-built for this Mac, from nixpkgs" }
} else if crate::index::ARM_LINUX {
    "over 80,000 for ARM Linux, from nixpkgs"
} else {
    "over 140,000, from nixpkgs"
};

fn tools() -> Vec<Value> {
    vec![
        tool("search", "Search Packages", &format!("Find apps and tools in nami's catalog ({CATALOG_SAID}). Apps come first; all includes command-line tools and libraries. Gives each one's name to install, its version and a line about it."), json!({
            "query": { "type": "string", "description": "What to look for: a name or what it does. Empty: the most used apps." },
            "all": { "type": "boolean", "default": false, "description": "Tools and libraries too, not just apps." },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
        }), &["query"], true, false),
        tool("info", "Package Details", "Details of packages by name: version, description, homepage, license, and whether it's installed.", json!({
            "packages": packages_schema("Names, as search gives them (firefox), or a version of one (firefox@150)."),
        }), &["packages"], true, false),
        tool("show", "Show the Whole Computer", "The whole computer in one file: system.nami merged with everything it includes, each line saying which file it came from. Secrets stay sealed.", json!({}), &[], true, false),
        tool("get", "Read a Setting", "What a nami file says at a path (all of it without one), as JSON: null when it says nothing there.", json!({
            "file": which_file(),
            "path": { "type": "string", "description": "Dotted, as `nami config` takes it: packages, env.EDITOR, namios.appearance.theme. Leave out for the whole file." },
        }), &[], true, false),
        tool("check", "Check a File", "Find problems in a nami file without applying it: unknown packages and options, mistakes, with line numbers.", json!({
            "file": which_file(),
        }), &[], true, false),
        tool("diff", "Compare Changes", "What changed between two changes (numbers from history), in words and as the file's lines. Without numbers: the last change.", json!({
            "from": { "type": "integer", "minimum": 0 },
            "to": { "type": "integer", "minimum": 0 },
        }), &[], true, false),
        tool("history", "History", "Every change, newest last, with what each added, removed or updated. Each can be gone back to with undo.", json!({
            "mine": { "type": "boolean", "default": true, "description": "Your own changes (true), or the computer's (false)." },
        }), &[], true, false),
        tool("install", "Install Apps", "Install apps and tools for you: they're added to your ~/user.nami and installed at once. What sets up the computer for everyone (a service, a driver: docker, steam, tailscale) can't be installed this way: propose it.", json!({
            "packages": packages_schema("Names from search (ripgrep), flathub:<app id>, or a version (nodejs@22)."),
        }), &["packages"], false, false),
        tool("remove", "Remove Apps", "Remove apps you installed: from your ~/user.nami, and from your setup at once. The system's apps (for everyone) can't be removed this way: propose it.", json!({
            "packages": packages_schema("Names as installed."),
        }), &["packages"], false, true),
        tool("config", "Change Your Setup", "Change your ~/user.nami in place, keeping its comments: your shell, environment, aliases, dotfiles, desktop settings, apps. Nothing changes on the computer until apply.", json!({
            "edit": edit_schema(),
        }), &["edit"], false, true),
        tool("apply", "Apply Your Setup", "Make your setup match your ~/user.nami: installs, removes and links what changed. Each apply is a change in history, which undo goes back from.", json!({
            "dry_run": { "type": "boolean", "default": false, "description": "Only say what would change." },
        }), &[], false, true),
        tool("undo", "Undo", "Go back to the change before this one (or to the change given): your setup and your ~/user.nami as they were then.", json!({
            "change": { "type": "integer", "minimum": 0, "description": "The change to go back to, from history. Default: the one before." },
        }), &[], false, true),
        tool("propose", "Propose a Computer Change", "Ask an administrator to change system.nami, the system's file (/etc/nami/system.nami: apps for everyone, services, hardware, the firewall, people). It's checked, then shown to them in Settings with your reason, the change in words and the lines; they approve it with their password or decline. Nothing changes until they do. Use wait to get the decision; else look it up with proposal.", json!({
            "edit": edit_schema(),
            "reason": { "type": "string", "minLength": 1, "maxLength": 600, "description": "Why, in a sentence, for the person deciding: what it's for." },
            "wait": { "type": "boolean", "default": false, "description": "Wait for the decision (up to timeout seconds)." },
            "timeout": { "type": "integer", "minimum": 1, "maximum": 3600, "default": 300 },
        }), &["edit", "reason"], false, false),
        tool("proposal", "Proposal Status", "Whether a proposal was approved and applied, declined, or is still waiting; what it changes, and any problem with it.", json!({
            "id": { "type": "string", "description": "The id propose gave." },
        }), &["id"], true, false),
    ]
}

/// What a run of nami said.
struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

fn program() -> PathBuf {
    crate::cmd::daemon::daemon_program()
}

/// Run nami, as the person, with no terminal: nothing asks for anything.
fn nami(args: &[String], input: Option<&str>) -> Ran {
    let mut cmd = Command::new(program());
    cmd.args(["--color", "never"]).args(args).env("NO_COLOR", "1").env_remove("CLICOLOR_FORCE").stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    let child = cmd.spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Ran { code: 127, stdout: String::new(), stderr: format!("can't run nami: {e}") },
    };
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) => Ran { code: o.status.code().unwrap_or(1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() },
        Err(e) => Ran { code: 1, stdout: String::new(), stderr: e.to_string() },
    }
}

/// The last lines of what nami said, without its "error: ".
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| l.strip_prefix("error: ").unwrap_or(l)).collect::<Vec<_>>().join("\n")
}

/// A tool's result: text, and the JSON as structured content (an object) when there's JSON.
fn result(text: String, structured: Option<Value>, is_error: bool) -> Value {
    let mut r = Map::new();
    r.insert("content".into(), json!([{ "type": "text", "text": text }]));
    if let Some(s) = structured {
        let s = if s.is_object() { s } else { json!({ "result": s }) };
        r.insert("structuredContent".into(), s);
    }
    r.insert("isError".into(), Value::Bool(is_error));
    Value::Object(r)
}

fn failed(message: impl Into<String>) -> Value {
    result(message.into(), None, true)
}

/// A run's result: its JSON when it printed some, else what it said.
fn from_run(r: Ran) -> Value {
    if r.code != 0 {
        let said = tail(&r.stderr, 12);
        return failed(if said.is_empty() { format!("nami stopped (exit {})", r.code) } else { said });
    }
    match serde_json::from_str::<Value>(r.stdout.trim()) {
        Ok(v) => result(r.stdout.trim().to_string(), Some(v), false),
        Err(_) => {
            let mut text = r.stdout.trim_end().to_string();
            let said = tail(&r.stderr, 40);
            if !said.is_empty() {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&said);
            }
            result(if text.is_empty() { "Done.".into() } else { text }, None, false)
        }
    }
}

fn file_arg(args: &Value) -> Result<PathBuf, String> {
    match args.get("file").and_then(Value::as_str).unwrap_or("user") {
        "user" => Ok(crate::config::user_file()),
        "computer" => Ok(crate::config::system_file()),
        other => Err(format!("file is user or computer, not {other:?}")),
    }
}

fn strings(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let list = args.get(key).and_then(Value::as_array).ok_or_else(|| format!("{key}: a list of names"))?;
    let names: Vec<String> = list.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    if names.is_empty() || names.len() != list.len() {
        return Err(format!("{key}: a list of names"));
    }
    // (Names, not options: nothing here starts with a dash.)
    if let Some(bad) = names.iter().find(|n| n.starts_with('-')) {
        return Err(format!("{bad:?} isn't a package name"));
    }
    Ok(names)
}

/// The edits given, as a list of JSON edits.
fn edits(args: &Value) -> Result<Vec<Value>, String> {
    let given = args.get("edit").ok_or("edit: an edit, or a list of them")?;
    let list = match given {
        Value::Array(l) => l.clone(),
        one => vec![one.clone()],
    };
    if list.is_empty() {
        return Err("edit: an edit, or a list of them".into());
    }
    for e in &list {
        crate::config_edit::Op::from_json(&e.to_string()).map_err(|e| format!("{e:#}"))?;
    }
    Ok(list)
}

fn call(session: &Session, name: &str, args: &Value) -> Value {
    match run_tool(session, name, args) {
        Ok(v) => v,
        Err(e) => failed(e),
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

fn run_tool(session: &Session, name: &str, args: &Value) -> Result<Value, String> {
    let flag = |k: &str| args.get(k).and_then(Value::as_bool);
    let int = |k: &str| args.get(k).and_then(Value::as_u64);
    Ok(match name {
        "search" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
            let mut a = vec![s("search"), query.to_string(), s("--json")];
            if flag("all") == Some(true) {
                a.push(s("--all"));
            }
            let r = nami(&a, None);
            if r.code != 0 {
                return Ok(from_run(r));
            }
            let limit = int("limit").unwrap_or(20).clamp(1, 100) as usize;
            let mut found: Value = serde_json::from_str(r.stdout.trim()).map_err(|e| format!("nami search: {e}"))?;
            let more = found.as_array().map_or(0, |l| l.len().saturating_sub(limit));
            if let Some(l) = found.as_array_mut() {
                l.truncate(limit);
            }
            let v = json!({ "results": found, "more": more });
            result(v.to_string(), Some(v), false)
        }
        "info" => {
            let mut a = vec![s("info")];
            a.extend(strings(args, "packages")?);
            a.push(s("--json"));
            from_run(nami(&a, None))
        }
        "show" => from_run(nami(&[s("show"), s("--flat")], None)),
        "get" => {
            let file = file_arg(args)?;
            let mut a = vec![s("config"), s("get")];
            if let Some(p) = args.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) {
                a.push(p.trim().to_string());
            }
            a.extend([s("--json"), s("--file"), file.display().to_string()]);
            from_run(nami(&a, None))
        }
        "check" => {
            let file = file_arg(args)?;
            let r = nami(&[s("check"), file.display().to_string(), s("--json")], None);
            // (Problems found: still what was asked for.)
            match serde_json::from_str::<Value>(r.stdout.trim()) {
                Ok(problems) => {
                    let v = json!({ "file": file, "ok": r.code == 0, "problems": problems });
                    result(v.to_string(), Some(v), false)
                }
                Err(_) => from_run(r),
            }
        }
        "diff" => {
            let mut a = vec![s("diff")];
            match (int("from"), int("to")) {
                (Some(f), Some(t)) => a.extend([f.to_string(), t.to_string()]),
                (Some(f), None) => a.push(f.to_string()),
                (None, Some(_)) => return Err("to needs from too".into()),
                (None, None) => {}
            }
            a.push(s("--json"));
            from_run(nami(&a, None))
        }
        "history" => {
            let mut a = vec![s("history"), s("--json")];
            if flag("mine") == Some(false) {
                a.push(s("--system"));
            }
            from_run(nami(&a, None))
        }
        "install" => {
            let names = strings(args, "packages")?;
            let computers: Vec<&String> = names.iter().filter(|p| !crate::flathub::is_flathub(p) && crate::new::sets_up_system(p)).collect();
            if let Some(p) = computers.first() {
                return Err(format!("{p} sets up the computer (a service, files in /etc), so it's installed for everyone, in system.nami, which an administrator approves. Use propose with {{\"add\": {{\"path\": [\"packages\"], \"value\": \"{p}\"}}}}."));
            }
            let mut a = vec![s("install")];
            a.extend(names);
            from_run(nami(&a, None))
        }
        "remove" => {
            let names = strings(args, "packages")?;
            // An admin's removal of what isn't in their own file goes to the system's apps.
            if crate::profile::is_admin() && !crate::profile::is_root() {
                let file = crate::config::user_file();
                let text = std::fs::read_to_string(&file).unwrap_or_default();
                let dir = file.parent().map(PathBuf::from).unwrap_or_default();
                if let Some(p) = names.iter().find(|p| !crate::flathub::is_flathub(p) && crate::config::remove_package_in(&text, &crate::cmd::packages::config_name(p, &dir), &dir).is_empty()) {
                    return Err(format!("{p} isn't in your ~/user.nami: it's one of the system's apps, for everyone. Use propose with {{\"remove\": {{\"path\": [\"packages\"], \"value\": \"{p}\"}}}}."));
                }
            }
            let mut a = vec![s("remove")];
            a.extend(names);
            from_run(nami(&a, None))
        }
        "config" => {
            let file = crate::config::user_file();
            let mut said = Vec::new();
            for e in edits(args)? {
                let r = nami(&[s("config"), s("json"), e.to_string(), s("--file"), file.display().to_string()], None);
                if r.code != 0 {
                    let done = if said.is_empty() { String::new() } else { format!(" ({} edit{} before it made)", said.len(), if said.len() == 1 { " was" } else { "s were" }) };
                    return Err(format!("{}{done}", tail(&r.stderr, 6)));
                }
                said.push(e);
            }
            let v = json!({ "file": file, "edits": said.len(), "next": "apply makes it so" });
            result(format!("Changed {} ({} edit{}). Run apply to make it so.", file.display(), said.len(), if said.len() == 1 { "" } else { "s" }), Some(v), false)
        }
        "apply" => {
            let mut a = vec![s("apply"), crate::config::user_file().display().to_string()];
            if flag("dry_run") == Some(true) {
                a.push(s("--dry-run"));
            }
            from_run(nami(&a, None))
        }
        "undo" => {
            let mut a = vec![s("undo")];
            if let Some(c) = int("change") {
                a.push(c.to_string());
            }
            from_run(nami(&a, None))
        }
        "propose" => {
            let edits = edits(args)?;
            let reason = args.get("reason").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty()).ok_or("reason: why, in a sentence, for the person deciding")?;
            let by = session.by.clone().unwrap_or_else(|| "An app".into());
            let mut a = vec![s("propose"), Value::Array(edits).to_string(), s("--reason"), reason.to_string(), s("--by"), by, s("--json")];
            if flag("wait") == Some(true) {
                a.extend([s("--wait"), s("--timeout"), int("timeout").unwrap_or(PROPOSE_WAIT).clamp(1, 3600).to_string()]);
            }
            let r = nami(&a, None);
            if r.code != 0 {
                return Ok(from_run(r));
            }
            let v: Value = serde_json::from_str(r.stdout.trim()).map_err(|e| format!("nami propose: {e}"))?;
            let v = brief(&v);
            let text = match v["status"].as_str() {
                Some("pending") => format!("Proposal {} is waiting for an administrator, who sees it in Settings. Look it up later with proposal.\n\n{}", v["id"].as_str().unwrap_or_default(), v),
                _ => v.to_string(),
            };
            result(text, Some(v), false)
        }
        "proposal" => {
            let id = args.get("id").and_then(Value::as_str).filter(|i| crate::proposals::valid_id(i)).ok_or("id: as propose gave it")?;
            let r = nami(&[s("proposals"), s("show"), id.to_string(), s("--json")], None);
            if r.code != 0 {
                return Ok(from_run(r));
            }
            let v: Value = serde_json::from_str(r.stdout.trim()).map_err(|e| format!("nami proposals: {e}"))?;
            let v = brief(&v);
            result(v.to_string(), Some(v), false)
        }
        _ => return Err(format!("no tool named {name:?}")),
    })
}

/// A proposal as an agent needs it: its id, status, what it changes and what's wrong with it.
fn brief(v: &Value) -> Value {
    let mut out = Map::new();
    for k in ["id", "status", "message", "summary", "stale"] {
        if let Some(x) = v.get(k).filter(|x| !x.is_null()) {
            out.insert(k.into(), x.clone());
        }
    }
    if let Some(raw) = v["diff"]["raw"].as_str() {
        out.insert("lines".into(), Value::String(raw.to_string()));
    }
    if let Some(p) = v.get("problems").and_then(Value::as_array).filter(|p| !p.is_empty()) {
        out.insert("problems".into(), Value::Array(p.clone()));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_hello_and_lists_its_tools() {
        let mut s = Session::default();
        let r = handle(&mut s, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2.0"}}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "nami");
        assert_eq!(s.by.as_deref(), Some("Claude"));
        // A version it doesn't know: its newest.
        let r = handle(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], VERSIONS[0]);
        assert!(handle(&mut s, json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let r = handle(&mut s, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).unwrap();
        let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["search", "info", "show", "get", "check", "diff", "history", "install", "remove", "config", "apply", "undo", "propose", "proposal"]);
        for t in r["result"]["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().unwrap().len() > 40, "{}", t["name"]);
        }
        assert_eq!(handle(&mut s, json!({"jsonrpc": "2.0", "id": 4, "method": "nope"})).unwrap()["error"]["code"], -32601);
        assert_eq!(handle(&mut s, json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "rm", "arguments": {}}})).unwrap()["error"]["code"], -32602);
        assert_eq!(handle(&mut s, json!({"jsonrpc": "2.0", "id": 6, "method": "ping"})).unwrap()["result"], json!({}));
    }

    #[test]
    fn what_a_tool_refuses_before_running_anything() {
        let s = Session::default();
        // Not names; not edits; no reason; an id that's a path.
        assert!(run_tool(&s, "install", &json!({"packages": ["--for-everyone"]})).is_err());
        assert!(run_tool(&s, "install", &json!({"packages": []})).is_err());
        assert!(run_tool(&s, "config", &json!({"edit": {"frobnicate": {}}})).is_err());
        assert!(run_tool(&s, "propose", &json!({"edit": {"add": {"path": ["packages"], "value": "docker"}}, "reason": " "})).is_err());
        assert!(run_tool(&s, "proposal", &json!({"id": "../x"})).is_err());
        assert!(run_tool(&s, "get", &json!({"file": "/etc/shadow"})).is_err());
        // Docker sets up the computer: proposed, not installed.
        let e = run_tool(&s, "install", &json!({"packages": ["docker"]})).unwrap_err();
        assert!(e.contains("propose"), "{e}");
    }
}
