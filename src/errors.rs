//! nami's errors for scripts and agents: with `--json`, a failed command prints one line,
//! `{"error": {"code": "...", "message": "...", "hint": "..."}}`, on stdout, as well as its
//! plain message on stderr; the exit code goes with the code. The codes are few and stay the
//! same from one nami to the next (`CODES`, and `nami help json` in the docs): a new kind of
//! failure gets one of these, not a new code.
//!
//! An error says its code where it's made (`coded`); one that doesn't is read by what it is
//! (an I/O error's kind, a YAML error) and, as it may have come through the daemon as text,
//! by its words.

use std::fmt;

/// What went wrong, in a word a program can match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    /// The command line or a value on it is wrong (clap's own errors too).
    InvalidArgs,
    /// A package, file, change, window, network or page that isn't there.
    NotFound,
    /// A nami file (system.nami, user.nami, project.nami...) has an error in it.
    InvalidFile,
    /// It changes the computer for everyone: an administrator's password is needed.
    NeedsAdmin,
    /// It would ask a question, and there's nobody to answer (no terminal, or
    /// NAMI_NONINTERACTIVE=1): give the answer as a flag (`--yes`, `--password-stdin`).
    NeedsInput,
    /// No connection, or a download failed: trying again later may work.
    Network,
    /// Something else is changing it, it changed meanwhile, or it's there already.
    Conflict,
    /// Refused: a wrong password, or not allowed.
    Denied,
    /// The namiOS desktop isn't running in this session (`nami desktop`).
    NotRunning,
    /// Not on this computer: no hardware for it, or not namiOS.
    Unsupported,
    /// Anything else.
    Failed,
}

/// Every code, with what it means: what the docs list.
pub const CODES: &[(Code, &str)] = &[
    (Code::InvalidArgs, "The command line or a value on it is wrong."),
    (Code::NotFound, "A package, file, change, window, network or page that isn't there."),
    (Code::InvalidFile, "A nami file has an error in it."),
    (Code::NeedsAdmin, "It changes the computer for everyone: an administrator's password is needed."),
    (Code::NeedsInput, "It would ask a question and nobody can answer: pass the answer as a flag (--yes, --password-stdin)."),
    (Code::Network, "No connection, or a download failed. Trying again later may work."),
    (Code::Conflict, "Something else is changing it, it changed meanwhile, or it already exists."),
    (Code::Denied, "Refused: a wrong password, or not allowed."),
    (Code::NotRunning, "The namiOS desktop isn't running in this session."),
    (Code::Unsupported, "Not on this computer: no hardware for it, or not namiOS."),
    (Code::Failed, "Anything else."),
];

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::InvalidArgs => "invalid_args",
            Code::NotFound => "not_found",
            Code::InvalidFile => "invalid_file",
            Code::NeedsAdmin => "needs_admin",
            Code::NeedsInput => "needs_input",
            Code::Network => "network",
            Code::Conflict => "conflict",
            Code::Denied => "denied",
            Code::NotRunning => "not_running",
            Code::Unsupported => "unsupported",
            Code::Failed => "failed",
        }
    }

    pub fn of_str(word: &str) -> Option<Code> {
        CODES.iter().map(|(c, _)| *c).find(|c| c.as_str() == word)
    }

    /// The exit code: 2 for the command line (as clap's), 75 for the network (EX_TEMPFAIL:
    /// later may work), 77 for an administrator's (EX_NOPERM), 1 for the rest.
    pub fn exit_code(self) -> i32 {
        match self {
            Code::InvalidArgs => 2,
            Code::Network => 75,
            Code::NeedsAdmin => 77,
            _ => 1,
        }
    }

    /// What to do about it, when the error doesn't say.
    fn hint(self) -> Option<&'static str> {
        match self {
            Code::NeedsAdmin => Some("Run it as an administrator, or ask one; Settings asks for their password."),
            Code::NeedsInput => Some("Pass the answer as a flag (--yes, --password-stdin); see the command's --help."),
            Code::Network => Some("Check the connection and try again."),
            Code::InvalidFile => Some("`nami check <file>` lists what's wrong, with line numbers."),
            Code::NotRunning => Some("Run it in the namiOS session of the person logged in (XDG_RUNTIME_DIR set)."),
            _ => None,
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error that says its code (and maybe what to do).
#[derive(Debug)]
pub struct Coded {
    pub code: Code,
    pub message: String,
    pub hint: Option<String>,
}

impl fmt::Display for Coded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Coded {}

/// An error with its code.
pub fn coded(code: Code, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Coded { code, message: message.into(), hint: None })
}

/// The same, with what to do.
pub fn coded_hint(code: Code, message: impl Into<String>, hint: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Coded { code, message: message.into(), hint: Some(hint.into()) })
}

/// Whether an error is the network's (no connection, no name lookup yet): by its words, as it
/// may have come through the daemon as text.
pub fn offline(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "dns", "resolve", "name or service not known", "temporary failure in name resolution", "network is unreachable", "connection refused", "connection reset", "timed out", "could not connect", "failed to connect", "no route to host", "couldn't reach",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// An error as people read it: the system's words for the common ones in
/// plain ones (`No such file or directory (os error 2)`: it doesn't exist),
/// a cause said once, not twice; and what to do, when there's something.
pub fn plain(text: &str, hint: Option<String>) -> (String, Option<String>) {
    let mut t = text.to_string();
    // `(os error 2)`: the system's number, for no one.
    while let Some(at) = t.find(" (os error ") {
        match t[at..].find(')') {
            Some(end) => t.replace_range(at..at + end + 1, ""),
            None => break,
        }
    }
    // The network, said as what it is.
    if let Some(host) = t.split("dns name '").nth(1).and_then(|r| r.split(['\'', ':']).next()).filter(|h| !h.is_empty()) {
        let what = t.split(": ").next().unwrap_or_default();
        let what = match what.strip_prefix("fetching ") {
            Some(url) => format!("can't download {}", url.rsplit('/').find(|s| !s.is_empty()).unwrap_or(url)),
            None => what.to_string(),
        };
        return (format!("{what}: couldn't reach {host}"), Some("Check that this computer is online, then try again.".into()));
    }
    // The daemon, there or not.
    if t.starts_with("connecting to the nami daemon") && (t.contains("Connection refused") || t.contains("No such file")) {
        return ("the nami daemon isn't running".into(), Some("Start it with `sudo systemctl start nami-daemon`.".into()));
    }
    for (system, plain) in [
        ("No such file or directory", "it doesn't exist"),
        ("Permission denied", "permission denied"),
        ("Read-only file system", "it can't be changed (a read-only disk)"),
        ("No space left on device", "the disk is full"),
        ("Directory not empty", "the folder isn't empty"),
        ("File exists", "it's already there"),
    ] {
        t = t.replace(system, plain);
    }
    // `reading x: it doesn't exist` -> `x doesn't exist`.
    for (verb, now) in [("reading ", "can't read "), ("writing ", "can't write "), ("opening ", "can't open ")] {
        if let Some(rest) = t.strip_prefix(verb) {
            t = match rest.split_once(": it doesn't exist") {
                Some((what, after)) if !what.contains(": ") => format!("{what} doesn't exist{after}"),
                _ => format!("{now}{rest}"),
            };
            break;
        }
    }
    // A cause said twice (`x: failed to look up: failed to look up`): once.
    let mut parts: Vec<&str> = Vec::new();
    for p in t.split(": ") {
        if p.len() < 12 || !parts.contains(&p) {
            parts.push(p);
        }
    }
    let t = parts.join(": ");
    let hint = hint.or_else(|| t.contains("permission denied").then(|| "It needs an administrator: try it with `sudo`.".to_string()));
    (t, hint)
}

/// An error's code, and what to do about it.
pub fn classify(e: &anyhow::Error) -> (Code, Option<String>) {
    for cause in e.chain() {
        if let Some(c) = cause.downcast_ref::<Coded>() {
            return (c.code, c.hint.clone().or_else(|| c.code.hint().map(str::to_string)));
        }
    }
    let code = e.chain().find_map(|cause| {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            return match io.kind() {
                NotFound => Some(Code::NotFound),
                PermissionDenied => Some(Code::Denied),
                AlreadyExists => Some(Code::Conflict),
                ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected | TimedOut | HostUnreachable | NetworkUnreachable | NetworkDown => Some(Code::Network),
                _ => None,
            };
        }
        if cause.downcast_ref::<serde_yaml::Error>().is_some() {
            return Some(Code::InvalidFile);
        }
        None
    });
    let code = code.unwrap_or_else(|| by_words(&format!("{e:#}")));
    (code, code.hint().map(str::to_string))
}

/// A code by an error's words alone (one that came through the daemon is only text).
pub fn by_words(text: &str) -> Code {
    let t = text.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if offline(text) || any(&["download", "http error", "status code 5", "certificate"]) && any(&["fail", "couldn't", "error"]) {
        Code::Network
    } else if any(&["administrator", "admin's password", "needs root", "as root", "run with sudo", "permission denied (are you root"]) {
        Code::NeedsAdmin
    } else if any(&["run in a terminal", "no terminal to ask", "pass --yes", "or pass --yes", "nobody to answer"]) {
        Code::NeedsInput
    } else if file_line(&t) || any(&["unknown key", "isn't valid yaml", "did not find expected", "mapping values are not allowed", "invalid type:", "expected a ", " line ", "at line"]) && any(&[".nami", "nami file", "yaml"]) {
        Code::InvalidFile
    } else if any(&["unknown package", "not installed", "doesn't exist", "isn't there", "no such file", "not found", "no package in the catalog has", "isn't in the catalog"]) {
        Code::NotFound
    } else if any(&["is locked", "already running", "another nami", "changed since", "already exists", "is in use"]) {
        Code::Conflict
    } else if any(&["wrong password", "not allowed", "refused", "permission denied"]) {
        Code::Denied
    } else {
        Code::Failed
    }
}

/// `user.nami:3: ...`: a nami file's line, as `nami check` and apply say a mistake in one.
fn file_line(text: &str) -> bool {
    text.match_indices(".nami:").any(|(at, m)| text[at + m.len()..].chars().next().is_some_and(|c| c.is_ascii_digit()))
}

/// The JSON a failed command prints with --json.
pub fn json_of(e: &anyhow::Error) -> serde_json::Value {
    let (code, hint) = classify(e);
    let message = crate::config::plain_message(&format!("{e:#}"));
    shape(code, &message, hint.as_deref())
}

/// `{"error": {"code", "message", "hint"}}` (no hint: none there).
pub fn shape(code: Code, message: &str, hint: Option<&str>) -> serde_json::Value {
    let mut inner = serde_json::json!({ "code": code.as_str(), "message": message });
    if let Some(h) = hint {
        inner["hint"] = serde_json::Value::String(h.to_string());
    }
    serde_json::json!({ "error": inner })
}

static JSON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This run prints JSON (`--json`): what's printed deep down follows it (`nami apply --dry-run`).
pub fn set_json(on: bool) {
    JSON.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn json() -> bool {
    JSON.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether a command line asks for JSON (`--json` anywhere before a `--`).
pub fn wants_json(args: &[String]) -> bool {
    args.iter().skip(1).take_while(|a| *a != "--").any(|a| a == "--json")
}

/// Whether nami may ask anything: never with NAMI_NONINTERACTIVE=1 (or true, yes), nor
/// without a terminal to ask on.
pub fn may_ask() -> bool {
    !noninteractive() && unsafe { libc::isatty(0) } == 1
}

/// A line typed in the terminal without showing it (a password), after `prompt`.
pub fn read_hidden(prompt: &str) -> anyhow::Result<String> {
    use std::io::Write;
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    let got = unsafe { libc::tcgetattr(0, &mut term) } == 0;
    if got {
        let mut quiet = term;
        quiet.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &quiet) };
    }
    let mut line = String::new();
    let r = std::io::stdin().read_line(&mut line);
    if got {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &term) };
    }
    eprintln!();
    r?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// NAMI_NONINTERACTIVE set (to anything but 0, false, no or empty): nothing waits on a
/// question, here or in a window (the daemon asks polkit for no password box then).
pub fn noninteractive() -> bool {
    std::env::var("NAMI_NONINTERACTIVE").is_ok_and(|v| !matches!(v.trim().to_lowercase().as_str(), "" | "0" | "false" | "no"))
}

/// The error for a question nobody can answer.
pub fn needs_input(question: &str, flag: &str) -> anyhow::Error {
    coded_hint(Code::NeedsInput, format!("{question}: nobody to answer (no terminal, or NAMI_NONINTERACTIVE is set)"), format!("Pass {flag}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_one_word_and_back() {
        for (code, about) in CODES {
            assert_eq!(Code::of_str(code.as_str()), Some(*code));
            assert!(!about.is_empty());
            assert!(code.as_str().chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{code}");
        }
        // (The words are what scripts match on: they stay.)
        let words: Vec<&str> = CODES.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(words, ["invalid_args", "not_found", "invalid_file", "needs_admin", "needs_input", "network", "conflict", "denied", "not_running", "unsupported", "failed"]);
    }

    #[test]
    fn the_shape_is_fixed() {
        let v = shape(Code::NotFound, "unknown package \"fooo\"", Some("`nami search fooo` looks for it."));
        assert_eq!(v, serde_json::json!({ "error": { "code": "not_found", "message": "unknown package \"fooo\"", "hint": "`nami search fooo` looks for it." } }));
        let v = shape(Code::Failed, "it broke", None);
        assert_eq!(v, serde_json::json!({ "error": { "code": "failed", "message": "it broke" } }));
        assert!(v["error"].get("hint").is_none());
    }

    #[test]
    fn a_coded_error_says_its_code_through_context() {
        let (t, h) = plain("reading /home/ada/user.nami: No such file or directory (os error 2)", None);
        assert_eq!((t.as_str(), h), ("/home/ada/user.nami doesn't exist", None));
        let (t, h) = plain("fetching https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz: https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz: Dns Failed: resolve dns name 'ftp.gnu.org:443': failed to lookup address information: Temporary failure in name resolution: failed to lookup address information: Temporary failure in name resolution", None);
        assert_eq!(t, "can't download hello-2.12.1.tar.gz: couldn't reach ftp.gnu.org");
        assert!(h.is_some());
        let (t, h) = plain("connecting to the nami daemon at /nix/var/nami/daemon.sock: Connection refused (os error 111)", None);
        assert_eq!(t, "the nami daemon isn't running");
        assert!(h.unwrap().contains("systemctl start nami-daemon"));
        let (t, h) = plain("writing /etc/x: Permission denied (os error 13)", None);
        assert_eq!(t, "can't write /etc/x: permission denied");
        assert!(h.unwrap().contains("sudo"));
        let (t, _) = plain("x: something went wrong: something went wrong", None);
        assert_eq!(t, "x: something went wrong");
        let e = coded_hint(Code::Conflict, "the store is busy", "Wait for the other nami.");
        let e = e.context("installing jq");
        let v = json_of(&e);
        assert_eq!(v["error"]["code"], "conflict");
        assert_eq!(v["error"]["hint"], "Wait for the other nami.");
        assert_eq!(v["error"]["message"], "installing jq: the store is busy");
    }

    #[test]
    fn errors_without_a_code_are_read_by_what_they_are() {
        let io = anyhow::Error::new(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")).context("reading ~/user.nami");
        assert_eq!(classify(&io).0, Code::NotFound);
        let yaml = serde_yaml::from_str::<serde_yaml::Value>("a: [").unwrap_err();
        assert_eq!(classify(&anyhow::Error::new(yaml)).0, Code::InvalidFile);
        assert_eq!(classify(&anyhow::anyhow!("unknown package \"fooo\"; did you mean: foo")).0, Code::NotFound);
        assert_eq!(classify(&anyhow::anyhow!("Not changed: that needs an admin's password.")).0, Code::NeedsAdmin);
        assert_eq!(classify(&anyhow::anyhow!("Temporary failure in name resolution")).0, Code::Network);
        assert_eq!(classify(&anyhow::anyhow!("user.nami: unknown key `pakages`")).0, Code::InvalidFile);
        assert_eq!(classify(&anyhow::anyhow!("system.nami:12: true or false, not the text \"yes\"")).0, Code::InvalidFile);
        assert_eq!(classify(&anyhow::anyhow!("writing it erases it: run in a terminal to say yes, or pass --yes")).0, Code::NeedsInput);
        assert_eq!(classify(&anyhow::anyhow!("something odd")).0, Code::Failed);
    }

    #[test]
    fn exit_codes_go_with_the_codes() {
        assert_eq!(Code::Network.exit_code(), 75);
        assert_eq!(Code::NeedsAdmin.exit_code(), 77);
        assert_eq!(Code::InvalidArgs.exit_code(), 2);
        assert_eq!(Code::NotFound.exit_code(), 1);
    }

    #[test]
    fn json_is_asked_for_before_a_double_dash() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(wants_json(&args(&["nami", "list", "--json"])));
        assert!(!wants_json(&args(&["nami", "run", "--", "tool", "--json"])));
        assert!(!wants_json(&args(&["nami", "list"])));
    }
}
