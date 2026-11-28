//! `nami doctor` on a computer nami made: whether what its packages set up
//! works, by the checks they bring (`checks.nami` next to a package.nami),
//! as tests/usb.sh runs them on a test computer. Each is a command and a
//! regular expression its output has to match.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;

/// How long one check may take.
const TIMEOUT: Duration = Duration::from_secs(60);

/// A check's result: its name, and what went wrong (its output's end).
pub struct Outcome {
    pub name: String,
    pub problem: Option<String>,
}

/// Run the checks of the computer `file` describes, and the ones every
/// computer has (no failed services). Checks that need the desktop ("in
/// the desktop: ...") run only from inside one.
pub fn run(file: &Path, mut each: impl FnMut(&Outcome)) -> Result<Vec<Outcome>> {
    let config = crate::config::load(file)?;
    let os = config.os.unwrap_or_default();
    let in_desktop = std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    let mut checks: Vec<(String, String, String)> = vec![
        ("no failed services".into(), "echo FAILED=$(systemctl --failed --no-legend --plain | wc -l); systemctl --failed --no-legend --plain".into(), "^FAILED=0$".into()),
        ("no failed services of yours".into(), "echo FAILED=$(systemctl --user --failed --no-legend --plain | wc -l); systemctl --user --failed --no-legend --plain".into(), "^FAILED=0$".into()),
    ];
    checks.extend(os.checks.iter().filter(|(name, _)| in_desktop || !name.starts_with("in the desktop:")).map(|(n, c)| (n.clone(), c.run.clone(), c.expect.clone())));
    let mut out = Vec::new();
    for (name, run, expect) in checks {
        let problem = match output(&run) {
            Ok(text) if matches(&text, &expect) => None,
            Ok(text) => Some(tail(&text)),
            Err(e) => Some(format!("{e:#}")),
        };
        let o = Outcome { name, problem };
        each(&o);
        out.push(o);
    }
    Ok(out)
}

/// What `run` prints (stdout and stderr), as `sh -c` runs it.
fn output(run: &str) -> Result<String> {
    let mut child = Command::new("sh").arg("-c").arg(format!("exec 2>&1; {run}")).stdin(Stdio::null()).stdout(Stdio::piped()).spawn()?;
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    while child.try_wait()?.is_none() {
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("no answer after {}s", TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(reader.join().unwrap_or_default())
}

/// Whether some line of `text` matches `expect`, as `grep -E` decides
/// (the tests' way, so a check means the same here and there).
fn matches(text: &str, expect: &str) -> bool {
    let mut grep = match Command::new("grep").args(["-qE", "--", expect]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if let Some(mut stdin) = grep.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
    grep.wait().is_ok_and(|s| s.success())
}

/// The last few lines of a check's output, for saying what it saw.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let shown = lines[lines.len().saturating_sub(6)..].join("\n");
    if shown.is_empty() {
        "(it printed nothing)".into()
    } else {
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_match_as_grep_does() {
        assert!(matches("FAILED=0\n", "^FAILED=0$"));
        assert!(!matches("FAILED=1\nx.service failed\n", "^FAILED=0$"));
        assert!(matches("x\nFAILED=0\n", "^FAILED=0$"), "any line matches");
        assert_eq!(output("echo hi; echo there >&2").unwrap(), "hi\nthere\n");
        assert_eq!(tail("\n\n"), "(it printed nothing)");
    }
}
