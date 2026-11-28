//! `nami mcp` and `nami propose`, as an agent uses them: the real program, in a scratch
//! computer (NAMI_ROOT, HOME and XDG_STATE_HOME in a temp folder), never this one.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

const COMPUTER: &str = "# Ada's laptop\npackages:\n  - firefox\n";

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("nami-mcp-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("root/etc/nami")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::write(dir.join("root/etc/nami/system.nami"), COMPUTER).unwrap();
        std::fs::write(dir.join("home/user.nami"), "packages:\n  - ripgrep\n").unwrap();
        Scratch(dir)
    }

    fn computer(&self) -> PathBuf {
        self.0.join("root/etc/nami/system.nami")
    }

    fn spool(&self) -> PathBuf {
        self.0.join("state/nami/proposals")
    }

    fn nami(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_nami"));
        c.env("NAMI_ROOT", self.0.join("root"))
            .env("HOME", self.0.join("home"))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("NO_COLOR", "1")
            // (No desktop to tell: no notification from the tests.)
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("XDG_RUNTIME_DIR");
        c
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Server {
    fn start(s: &Scratch) -> Server {
        let mut child = s.nami().arg("mcp").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Server { child, input, output, next: 0 }
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.input, "{msg}").unwrap();
        self.input.flush().unwrap();
    }

    fn ask(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"));
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], id);
        reply
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let r = self.ask("tools/call", json!({ "name": tool, "arguments": arguments }));
        assert!(r.get("error").is_none(), "{r}");
        r["result"].clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn proposals_in(spool: &Path) -> Vec<Value> {
    std::fs::read_dir(spool).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".json")).map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap()).collect()
}

#[test]
fn an_agent_says_hello_lists_the_tools_and_proposes_a_change() {
    let s = Scratch::new("handshake");
    let mut server = Server::start(&s);
    let init = server.ask("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "claude-code", "version": "2.1.0" } }));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "nami");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    let tools = server.ask("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["search", "info", "get", "check", "install", "remove", "config", "apply", "undo", "propose", "proposal"] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }

    // Reading the person's own file, and the computer's.
    let got = server.call("get", json!({ "path": "packages" }));
    assert_eq!(got["isError"], false, "{got}");
    assert_eq!(got["structuredContent"]["result"], json!(["ripgrep"]));
    let got = server.call("get", json!({ "file": "computer", "path": "packages" }));
    assert_eq!(got["structuredContent"]["result"], json!(["firefox"]), "{got}");

    // Their own file, changed with no one asked.
    let changed = server.call("config", json!({ "edit": { "set": { "path": ["env", "EDITOR"], "value": "hx" } } }));
    assert_eq!(changed["isError"], false, "{changed}");
    assert!(std::fs::read_to_string(s.0.join("home/user.nami")).unwrap().contains("EDITOR: hx"));

    // The computer's: proposed, kept, and the file untouched.
    let proposed = server.call("propose", json!({ "edit": { "add": { "path": ["packages"], "value": "vlc" } }, "reason": "Ada asked for a video player." }));
    assert_eq!(proposed["isError"], false, "{proposed}");
    let id = proposed["structuredContent"]["id"].as_str().unwrap().to_string();
    assert_eq!(proposed["structuredContent"]["status"], "pending");
    assert!(proposed["structuredContent"]["lines"].as_str().unwrap().contains("+  - vlc"), "{proposed}");
    assert_eq!(std::fs::read_to_string(s.computer()).unwrap(), COMPUTER);
    let kept = proposals_in(&s.spool());
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["by"], "Claude");
    assert_eq!(kept[0]["reason"], "Ada asked for a video player.");

    // A secret can't go in one.
    let secret = server.call("propose", json!({ "edit": { "set": { "path": ["env", "TOKEN"], "value": { "!secret": "abc" } } }, "reason": "A token." }));
    assert_eq!(secret["isError"], true, "{secret}");

    // Its status, until someone decides.
    let status = server.call("proposal", json!({ "id": id }));
    assert_eq!(status["structuredContent"]["status"], "pending", "{status}");
    let out = s.nami().args(["proposals", "reject", &id]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let status = server.call("proposal", json!({ "id": id }));
    assert_eq!(status["structuredContent"]["status"], "rejected", "{status}");
    assert_eq!(std::fs::read_to_string(s.computer()).unwrap(), COMPUTER);
}

#[test]
fn propose_waits_for_the_decision() {
    let s = Scratch::new("wait");
    let mut waiting = s.nami().args(["propose", r#"{"add": {"path": ["packages"], "value": "vlc"}}"#, "--reason", "A video player.", "--by", "Claude", "--wait", "--timeout", "60"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    // Its proposal, once it's there; then Don't Allow.
    let mut id = None;
    for _ in 0..100 {
        if let Some(p) = proposals_in(&s.spool()).first() {
            id = p["id"].as_str().map(String::from);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let id = id.expect("a proposal");
    let listed = s.nami().args(["proposals", "list", "--json"]).output().unwrap();
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed[0]["id"], id.as_str());
    assert_eq!(listed[0]["summary"], "Apps: + vlc");
    assert!(s.nami().args(["proposals", "reject", &id]).status().unwrap().success());
    let out = waiting.wait_with_output().unwrap();
    assert!(out.status.success());
    let said: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(said["id"], id.as_str());
    assert_eq!(said["status"], "rejected");
}

#[test]
fn a_stale_proposal_is_refused_with_a_clear_message() {
    let s = Scratch::new("stale");
    let out = s.nami().args(["propose", r#"{"add": {"path": ["packages"], "value": "vlc"}}"#, "--reason", "A video player.", "--by", "Claude", "--json", "--quiet"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();
    let (id, fingerprint) = (r["id"].as_str().unwrap().to_string(), r["fingerprint"].as_str().unwrap().to_string());
    // Shown as one thing, changed since: refused.
    let out = s.nami().args(["proposals", "apply", &id, "--expect", "0123"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("changed since it was shown"));
    // The computer's file changed underneath: refused, said plainly, and marked stale.
    std::fs::write(s.computer(), format!("{COMPUTER}  - gimp\n")).unwrap();
    let out = s.nami().args(["proposals", "apply", &id, "--expect", &fingerprint]).output().unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("system.nami changed since Claude asked: nothing was changed. Ask for the change again."), "{said}");
    assert_eq!(std::fs::read_to_string(s.computer()).unwrap(), format!("{COMPUTER}  - gimp\n"));
    let shown = s.nami().args(["proposals", "show", &id, "--json"]).output().unwrap();
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["status"], "stale");
    // Nothing that changes nothing, nor without a reason.
    let out = s.nami().args(["propose", r#"{"add": {"path": ["packages"], "value": "firefox"}}"#, "--reason", "Again.", "--quiet"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already"));
}
