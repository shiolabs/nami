//! Proposed changes to system.nami, for an administrator to approve.
//!
//! An agent (or any tool) running as someone can't change /etc/nami/system.nami: that
//! takes an administrator's password, which stays with the human. `nami propose` writes the
//! change down instead, with a short reason and who's asking, in the person's own spool
//! (`~/.local/state/nami/proposals/<id>.json`, theirs alone to read). Settings shows it
//! ("Claude wants to change your computer"): the reason, the change in words and the lines,
//! and Apply or Don't Allow.
//!
//! What keeps it safe:
//! - A proposal is only data. Nothing applies it but `nami proposals apply`, which writes the
//!   computer's file through nami's daemon, and the daemon asks polkit for an administrator's
//!   password then and there.
//! - It's checked again at apply time, against the file as it is then: the file it was made
//!   against (its sha256, `base`) must be the one there, or it's refused as stale. The daemon
//!   checks that too, as root, right before writing (`config write --expect`).
//! - What Settings showed is what's applied: Apply passes the proposal's fingerprint
//!   (`--expect`), and a proposal changed since (it's in a folder its maker can write) is refused.
//! - No secrets: a proposal sits in a plain file, so it can't carry one.

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config_edit::Op;
use crate::file_diff::{self, FileDiff};

/// What became of a proposal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting for someone to look at it.
    #[default]
    Pending,
    /// Approved, written and applied.
    Applied,
    /// Don't Allow.
    Rejected,
    /// system.nami changed since it was proposed: refused.
    Stale,
    /// Approved, but it couldn't be applied (the password wasn't given, the apply failed).
    Failed,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Applied => "applied",
            Status::Rejected => "rejected",
            Status::Stale => "stale",
            Status::Failed => "failed",
        }
    }
}

/// One proposed change to system.nami, as it's kept in the spool.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    /// Who's asking ("Claude"), as they say.
    pub by: String,
    /// Why, in their words.
    pub reason: String,
    /// When, in seconds.
    pub created: u64,
    /// The file it changes (the computer's).
    pub file: String,
    /// The file's sha256 when it was proposed.
    pub base: String,
    /// The edits, as `nami config json` takes them, in order.
    pub edits: Vec<Value>,
    #[serde(default)]
    pub status: Status,
    /// When it was decided, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<u64>,
    /// What happened, in a sentence (why it failed, or was refused).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The longest reason kept, and who's asking.
const REASON_MAX: usize = 600;
const BY_MAX: usize = 40;
/// Decided proposals are kept this long, for `--wait` and `list --all`.
const KEEP_DECIDED: u64 = 30 * 24 * 3600;

/// The person's spool: `$XDG_STATE_HOME/nami/proposals`, else `~/.local/state/nami/proposals`.
pub fn dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("nami/proposals")
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn digest(text: &str) -> String {
    crate::catalog::sha256_hex(text.as_bytes())
}

/// An id's shape: a few lowercase letters and digits (it names a file).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn new_id(dir: &Path) -> String {
    let seed = format!("{}-{}-{:?}", now(), std::process::id(), std::time::Instant::now());
    let hex = digest(&seed);
    // (Short, and unique in the spool.)
    (8..hex.len()).map(|n| hex[..n].to_string()).find(|id| !dir.join(format!("{id}.json")).exists()).unwrap_or(hex)
}

/// Text cut to `max` characters, on one line, without control characters.
fn tidy(text: &str, max: usize) -> String {
    let one: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        one.chars().take(max - 1).collect::<String>() + "…"
    } else {
        one
    }
}

/// The edits from what was given: one edit (`{"add": {...}}`) or a list of them.
pub fn edits_of(given: &str) -> Result<Vec<Value>> {
    let v: Value = serde_json::from_str(given).map_err(|e| anyhow::anyhow!("bad edit JSON: {e}"))?;
    let list = match v {
        Value::Array(l) => l,
        one => vec![one],
    };
    ensure!(!list.is_empty(), "no edits: give one, as `nami config json` takes it ({{\"add\": {{\"path\": [\"packages\"], \"value\": \"docker\"}}}})");
    ensure!(list.len() <= 50, "too many edits in one proposal (50 at most)");
    for e in &list {
        ensure!(!has_secret(e), "a proposal can't carry a secret: it's kept in a plain file. Add the secret to system.nami yourself (`nami config set ... '!secret ...'`).");
        Op::from_json(&e.to_string())?;
    }
    Ok(list)
}

fn has_secret(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.contains_key("!secret") || m.values().any(has_secret),
        Value::Array(l) => l.iter().any(has_secret),
        Value::String(s) => s.trim_start().starts_with("!secret"),
        _ => false,
    }
}

fn op_path(op: &Op) -> &[String] {
    match op {
        Op::Set { path, .. } | Op::Unset { path } | Op::Add { path, .. } | Op::Remove { path, .. } => path,
    }
}

/// The file's text with the edits made, in order, as `nami config json` makes each.
pub fn edited(text: &str, edits: &[Value]) -> Result<String> {
    let mut out = text.to_string();
    for e in edits {
        ensure!(!has_secret(e), "a proposal can't carry a secret");
        let op = Op::from_json(&e.to_string())?;
        out = crate::config_edit::apply(&crate::oldnames::keys_in_text(&out, op_path(&op)), &op)?;
    }
    Ok(out)
}

/// What Settings shows and Apply is checked by: the base and the edits, as one sha256.
pub fn fingerprint(p: &Proposal) -> String {
    digest(&format!("{}\n{}\n{}", p.file, p.base, serde_json::to_string(&p.edits).unwrap_or_default()))
}

/// A proposal looked at against the file as it is now.
#[derive(Debug, Serialize)]
pub struct Review {
    #[serde(flatten)]
    pub proposal: Proposal,
    pub fingerprint: String,
    /// The file changed since it was proposed.
    pub stale: bool,
    /// What it changes, in words and as lines (none when it's stale or can't be made).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<FileDiff>,
    /// What `nami check` finds in the file as it would be.
    pub problems: Vec<crate::config::Diagnostic>,
    /// The change in a line ("Apps: + docker").
    pub summary: String,
}

impl Review {
    pub fn errors(&self) -> Vec<&crate::config::Diagnostic> {
        self.problems.iter().filter(|d| d.severity == crate::config::Severity::Error).collect()
    }
}

/// The change in a line, for the password's box and the list: "Apps: + docker; Firewall: on → off".
pub fn summary(d: &FileDiff) -> String {
    let mut parts = Vec::new();
    for s in &d.sections {
        let rows: Vec<String> = s
            .rows
            .iter()
            .map(|r| {
                let words = file_diff::row_words(&s.key, r);
                match r.mark {
                    file_diff::Mark::Added => format!("+ {words}"),
                    file_diff::Mark::Removed => format!("− {words}"),
                    file_diff::Mark::Changed => words,
                }
            })
            .collect();
        parts.push(format!("{}: {}", s.title, rows.join(", ")));
    }
    if parts.is_empty() && !d.raw.is_empty() {
        parts.push("only comments change".into());
    }
    tidy(&parts.join("; "), 200)
}

/// What the password's box says: "Claude asked to add docker to Apps and docker.socket to
/// Services." In nami's words, from the change itself (not the asker's reason), and short.
pub fn asked(by: &str, d: &FileDiff) -> String {
    let mut clauses: Vec<String> = Vec::new();
    for s in &d.sections {
        for r in &s.rows {
            let words = file_diff::row_words(&s.key, r);
            clauses.push(match r.mark {
                file_diff::Mark::Added => format!("add {words} to {}", s.title),
                file_diff::Mark::Removed => format!("remove {words} from {}", s.title),
                file_diff::Mark::Changed => format!("change {words}"),
            });
        }
    }
    // (The same verb once: "add docker to Apps and docker.socket to Services".)
    for i in (1..clauses.len()).rev() {
        let verb = clauses[i - 1].split(' ').next().unwrap_or_default().to_string();
        if let Some(rest) = clauses[i].strip_prefix(&format!("{verb} ")) {
            clauses[i] = rest.to_string();
        }
    }
    let list = match clauses.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    if list.is_empty() || list.chars().count() > 140 {
        let n = d.sections.iter().map(|s| s.rows.len()).sum::<usize>();
        return format!("{by} asked to make {} to this computer's settings.", if n == 1 { "a change".to_string() } else { format!("{n} changes") });
    }
    format!("{by} asked to {list}.")
}

/// The problems the change brings: those of the file after it that it didn't have before (a
/// mistake already there isn't the proposal's, and doesn't stop it). By what they say, as
/// their lines move.
fn introduced(check: impl Fn(&str) -> Vec<crate::config::Diagnostic>, before: &str, after: &str) -> Vec<crate::config::Diagnostic> {
    let had: Vec<String> = check(before).into_iter().map(|d| d.message).collect();
    check(after).into_iter().filter(|d| d.severity != crate::config::Severity::Note && !had.contains(&d.message)).collect()
}

/// `p` against the file's text now (`current`); `check` finds the problems of a text.
pub fn review(p: Proposal, current: &str, check: impl Fn(&str) -> Vec<crate::config::Diagnostic>) -> Review {
    let fingerprint = fingerprint(&p);
    let stale = digest(current) != p.base;
    let mut problems = Vec::new();
    let mut diff = None;
    if !stale {
        match edited(current, &p.edits) {
            Ok(after) => {
                problems = introduced(&check, current, &after);
                diff = Some(file_diff::file("system.nami", current, &after, "now", "proposed"));
            }
            Err(e) => problems.push(crate::config::Diagnostic { line: 0, column: 0, severity: crate::config::Severity::Error, message: format!("{e:#}"), hint: None }),
        }
    }
    let summary = diff.as_ref().map(summary).unwrap_or_default();
    Review { proposal: p, fingerprint, stale, diff, problems, summary }
}

/// Write `p` to the spool, readable by its owner alone (in place at once: never half there).
pub fn save(dir: &Path, p: &Proposal) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    ensure!(valid_id(&p.id), "bad proposal id {:?}", p.id);
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).with_context(|| format!("making {}", dir.display()))?;
    let tmp = dir.join(format!(".{}.{}.tmp", p.id, std::process::id()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(serde_json::to_string_pretty(p)?.as_bytes())?;
    f.write_all(b"\n")?;
    drop(f);
    std::fs::rename(&tmp, dir.join(format!("{}.json", p.id)))?;
    Ok(())
}

pub fn load(dir: &Path, id: &str) -> Result<Proposal> {
    ensure!(valid_id(id), "no proposal {id:?} (`nami proposals list`)");
    let file = dir.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&file).map_err(|_| anyhow::anyhow!("no proposal {id} (`nami proposals list`)"))?;
    let p: Proposal = serde_json::from_str(&text).with_context(|| format!("reading {}", file.display()))?;
    // (A file renamed by hand is another's: by what it says it is.)
    ensure!(p.id == id, "proposal {id} says it's {}: not used", p.id);
    Ok(p)
}

/// Every proposal in the spool, oldest first; those decided long ago are cleared away.
pub fn list(dir: &Path) -> Vec<Proposal> {
    let mut all = Vec::new();
    let cutoff = now().saturating_sub(KEEP_DECIDED);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json").filter(|i| valid_id(i)) else { continue };
        let Ok(p) = load(dir, id) else { continue };
        if p.status != Status::Pending && p.decided.unwrap_or(p.created) < cutoff {
            let _ = std::fs::remove_file(e.path());
            continue;
        }
        all.push(p);
    }
    all.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
    all
}

/// A new proposal: the edits checked against the file's text now (`current`, at `file`);
/// refused if they can't be made, change nothing, or leave a mistake in the file.
pub fn create(dir: &Path, file: &Path, current: &str, edits: Vec<Value>, reason: &str, by: &str, check: impl Fn(&str) -> Vec<crate::config::Diagnostic>) -> Result<Review> {
    let reason = tidy(reason, REASON_MAX);
    ensure!(!reason.is_empty(), "say why, in a sentence (--reason): the person approving it reads it");
    let by = tidy(by, BY_MAX);
    let by = if by.is_empty() { "An app".to_string() } else { by };
    let after = edited(current, &edits).context("the change can't be made to system.nami")?;
    ensure!(after != current, "system.nami already says that: nothing to change");
    let p = Proposal { id: new_id(dir), by, reason, created: now(), file: file.display().to_string(), base: digest(current), edits, status: Status::Pending, decided: None, message: None };
    let r = review(p, current, check);
    let errors = r.errors();
    if !errors.is_empty() {
        let said: Vec<String> = errors.iter().map(|d| if d.line > 0 { format!("line {}: {}", d.line, d.message) } else { d.message.clone() }).collect();
        bail!("the change would leave a mistake in system.nami: {}", said.join("; "));
    }
    save(dir, &r.proposal)?;
    Ok(r)
}

/// Mark `id` decided: `status`, with what happened. Only a pending one is decided.
pub fn decide(dir: &Path, id: &str, status: Status, message: Option<String>) -> Result<Proposal> {
    let mut p = load(dir, id)?;
    ensure!(p.status == Status::Pending, "proposal {id} was {} already", p.status.word());
    p.status = status;
    p.decided = Some(now());
    p.message = message;
    save(dir, &p)?;
    Ok(p)
}

/// Before applying: `p` as Settings showed it (`expect`, its fingerprint), still pending, made
/// against the file as it is now, and still sound. The file's text after it, or why not; a
/// stale one is marked so.
pub fn ready(dir: &Path, p: &Proposal, expect: Option<&str>, current: &str, check: impl Fn(&str) -> Vec<crate::config::Diagnostic>) -> Result<String> {
    ensure!(p.status == Status::Pending, "proposal {} was {} already", p.id, p.status.word());
    if let Some(e) = expect {
        ensure!(e == fingerprint(p), "proposal {} changed since it was shown: nothing was changed. Look at it again.", p.id);
    }
    if digest(current) != p.base {
        let msg = format!("system.nami changed since {} asked: nothing was changed. Ask for the change again.", p.by);
        let _ = decide(dir, &p.id, Status::Stale, Some(msg.clone()));
        bail!("{msg}");
    }
    let after = edited(current, &p.edits)?;
    let problems = introduced(&check, current, &after);
    let errors: Vec<String> = problems.iter().filter(|d| d.severity == crate::config::Severity::Error).map(|d| d.message.clone()).collect();
    ensure!(errors.is_empty(), "the change would leave a mistake in system.nami: {}", errors.join("; "));
    Ok(after)
}

/// Wait for `id` to be decided (at most `timeout` seconds; 0: as long as it takes).
pub fn wait(dir: &Path, id: &str, timeout: u64) -> Result<Proposal> {
    let end = (timeout > 0).then(|| std::time::Instant::now() + std::time::Duration::from_secs(timeout));
    loop {
        let p = load(dir, id)?;
        if p.status != Status::Pending || end.is_some_and(|e| std::time::Instant::now() >= e) {
            return Ok(p);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// What `--wait` and `--json` print of a proposal.
pub fn result_json(p: &Proposal) -> Value {
    json!({ "id": p.id, "status": p.status.word(), "by": p.by, "reason": p.reason, "message": p.message, "created": p.created, "decided": p.decided })
}

/// "Claude wants to change your computer".
pub fn headline(by: &str) -> String {
    format!("{by} wants to change your computer")
}

/// Who an MCP client is, as a person reads it: "Claude" for Claude's apps.
pub fn client_name(name: &str, title: Option<&str>) -> String {
    let n = name.to_ascii_lowercase();
    if n.starts_with("claude") {
        return "Claude".into();
    }
    let known = [("cursor", "Cursor"), ("vscode", "VS Code"), ("visual-studio-code", "VS Code"), ("zed", "Zed"), ("codex", "Codex"), ("gemini", "Gemini"), ("windsurf", "Windsurf"), ("goose", "Goose")];
    if let Some((_, said)) = known.iter().find(|(k, _)| n.starts_with(k)) {
        return said.to_string();
    }
    let t = tidy(title.filter(|t| !t.trim().is_empty()).unwrap_or(name), BY_MAX);
    if t.is_empty() { "An app".into() } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Diagnostic, Severity};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nami-proposals-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn fine(_: &str) -> Vec<Diagnostic> {
        Vec::new()
    }

    const FILE: &str = "# the office laptop\npackages:\n  - firefox\n";

    fn add(pkg: &str) -> Vec<Value> {
        edits_of(&format!(r#"{{"add": {{"path": ["packages"], "value": "{pkg}"}}}}"#)).unwrap()
    }

    #[test]
    fn a_proposal_is_kept_and_read_back() {
        let dir = temp("kept");
        let r = create(&dir, Path::new("/etc/nami/system.nami"), FILE, add("docker"), "To run the test database.", "Claude", fine).unwrap();
        assert!(!r.stale && r.problems.is_empty());
        assert_eq!(r.summary, "Apps: + docker");
        assert!(r.diff.as_ref().unwrap().raw.contains("+  - docker"));
        let p = load(&dir, &r.proposal.id).unwrap();
        assert_eq!((p.by.as_str(), p.reason.as_str(), p.status), ("Claude", "To run the test database.", Status::Pending));
        assert_eq!(p.base, digest(FILE));
        // Only its owner reads it.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join(format!("{}.json", p.id))).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(list(&dir).len(), 1);
        // Decided once, and only once.
        decide(&dir, &p.id, Status::Rejected, None).unwrap();
        assert_eq!(load(&dir, &p.id).unwrap().status, Status::Rejected);
        assert!(decide(&dir, &p.id, Status::Applied, None).is_err());
        assert_eq!(wait(&dir, &p.id, 1).unwrap().status, Status::Rejected);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_cant_be_proposed() {
        let dir = temp("refused");
        let file = Path::new("/etc/nami/system.nami");
        // No reason; nothing that changes; a mistake left in the file; a secret; bad JSON.
        assert!(create(&dir, file, FILE, add("docker"), "  ", "Claude", fine).is_err());
        assert!(create(&dir, file, FILE, add("firefox"), "Again.", "Claude", fine).unwrap_err().to_string().contains("already"));
        let broken = |t: &str| if t.contains("dokcer") { vec![Diagnostic { line: 3, column: 1, severity: Severity::Error, message: "no package named dokcer".into(), hint: None }] } else { Vec::new() };
        // (A mistake already there is not the proposal's: it goes ahead.)
        let old = |_: &str| vec![Diagnostic { line: 1, column: 1, severity: Severity::Error, message: "namios: is a person's".into(), hint: None }];
        assert!(create(&dir, file, FILE, add("vlc"), "A player.", "Claude", old).is_ok());
        assert!(create(&dir, file, FILE, add("dokcer"), "Typo.", "Claude", broken).unwrap_err().to_string().contains("no package named dokcer"));
        assert!(edits_of(r#"{"set": {"path": ["env", "TOKEN"], "value": {"!secret": "abc"}}}"#).unwrap_err().to_string().contains("secret"));
        assert!(edits_of(r#"{"set": {"path": ["env", "TOKEN"], "value": "!secret abc"}}"#).is_err());
        assert!(edits_of("[]").is_err());
        assert!(edits_of(r#"{"frobnicate": {}}"#).is_err());
        // Ids name files: nothing outside the spool.
        assert!(load(&dir, "../../etc/passwd").is_err());
        assert_eq!(list(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_password_box_says_what_changes_in_a_sentence() {
        let edits = edits_of(r#"[{"add": {"path": ["packages"], "value": "docker"}}, {"add": {"path": ["services"], "value": "docker.socket"}}]"#).unwrap();
        let d = file_diff::file("system.nami", FILE, &edited(FILE, &edits).unwrap(), "now", "proposed");
        assert_eq!(asked("Claude", &d), "Claude asked to add docker to Apps and docker.socket to Services.");
        let gone = edits_of(r#"{"remove": {"path": ["packages"], "value": "firefox"}}"#).unwrap();
        let d = file_diff::file("system.nami", FILE, &edited(FILE, &gone).unwrap(), "now", "proposed");
        assert_eq!(asked("Claude", &d), "Claude asked to remove firefox from Apps.");
    }

    #[test]
    fn several_edits_in_one() {
        let edits = edits_of(r#"[{"add": {"path": ["packages"], "value": "docker"}}, {"remove": {"path": ["packages"], "value": "firefox"}}]"#).unwrap();
        let after = edited(FILE, &edits).unwrap();
        assert_eq!(after, "# the office laptop\npackages:\n  - docker\n");
    }

    #[test]
    fn a_stale_proposal_is_refused() {
        let dir = temp("stale");
        let r = create(&dir, Path::new("/etc/nami/system.nami"), FILE, add("docker"), "Docker, please.", "Claude", fine).unwrap();
        let p = r.proposal;
        // As it was: ready, the file as it'd be.
        assert_eq!(ready(&dir, &p, Some(&fingerprint(&p)), FILE, fine).unwrap(), "# the office laptop\npackages:\n  - firefox\n  - docker\n");
        // Settings showed something else (the file was changed by hand since): refused.
        assert!(ready(&dir, &p, Some("0000"), FILE, fine).unwrap_err().to_string().contains("changed since it was shown"));
        // system.nami changed underneath: refused, and marked so.
        let now = format!("{FILE}  - vlc\n");
        assert!(review(p.clone(), &now, fine).stale);
        let e = ready(&dir, &p, None, &now, fine).unwrap_err().to_string();
        assert!(e.contains("system.nami changed since Claude asked"), "{e}");
        assert_eq!(load(&dir, &p.id).unwrap().status, Status::Stale);
        // Decided: not again.
        assert!(ready(&dir, &load(&dir, &p.id).unwrap(), None, FILE, fine).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checked_again_at_apply_time() {
        let dir = temp("again");
        let r = create(&dir, Path::new("/etc/nami/system.nami"), FILE, add("docker"), "Docker.", "Claude", fine).unwrap();
        // (The catalog no longer has it, say.)
        let gone = |t: &str| if t.contains("docker") { vec![Diagnostic { line: 4, column: 5, severity: Severity::Error, message: "no package docker".into(), hint: None }] } else { Vec::new() };
        assert!(ready(&dir, &r.proposal, None, FILE, gone).is_err());
        assert_eq!(load(&dir, &r.proposal.id).unwrap().status, Status::Pending);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn who_is_asking() {
        assert_eq!(client_name("claude-code", None), "Claude");
        assert_eq!(client_name("claude-ai", Some("Claude")), "Claude");
        assert_eq!(client_name("cursor-vscode", None), "Cursor");
        assert_eq!(client_name("my-agent", Some("My Agent")), "My Agent");
        assert_eq!(client_name("x\u{7}y", None), "x y");
        assert_eq!(headline("Claude"), "Claude wants to change your computer");
    }
}
