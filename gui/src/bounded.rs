//! Commands run from the windows: none can hold up whoever asked, and none is left behind.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// What it said, stopped (with what it started) after `limit`. None: it wouldn't start, or wouldn't end.
pub fn output(command: &mut Command, limit: Duration) -> Option<Output> {
    run(command.stdin(Stdio::null()), None, limit)
}

/// The same, told this on its input.
pub fn told(command: &mut Command, input: &str, limit: Duration) -> Option<Output> {
    run(command.stdin(Stdio::piped()), Some(input.to_string()), limit)
}

fn run(command: &mut Command, input: Option<String>, limit: Duration) -> Option<Output> {
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn().ok()?;
    let pid = child.id() as i32;
    let (done, finished) = mpsc::channel();
    // (Waited for elsewhere: what's stopped may have left something holding its output.)
    std::thread::spawn(move || {
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            let _ = stdin.write_all(input.as_bytes());
        }
        let _ = done.send(child.wait_with_output());
    });
    match finished.recv_timeout(limit) {
        Ok(out) => out.ok(),
        Err(_) => {
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            let _ = finished.recv_timeout(Duration::from_millis(200));
            None
        }
    }
}

/// What it printed; nothing, if it couldn't say in time.
pub fn text(program: &str, args: &[&str], limit: Duration) -> String {
    output(Command::new(program).args(args), limit).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// Done on a thread of its own, and waited for there: what it waits on holds up nothing else
/// waiting its turn on the windows' few background threads, nor do they hold it up (Settings'
/// look at the computer, while `nami` waits for the store an install has).
pub async fn apart<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (done, finished) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = done.send(work());
    });
    finished.await.ok()
}

/// Started, and nobody waits; collected when it ends, so it doesn't stay as a zombie. Whether it started.
pub fn go(command: &mut Command) -> bool {
    match command.stdin(Stdio::null()).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn says_what_it_said() {
        assert_eq!(text("sh", &["-c", "echo hello"], Duration::from_secs(5)), "hello\n");
    }

    #[test]
    fn hears_what_it_is_told() {
        let said = told(Command::new("cat").arg("-"), "hello", Duration::from_secs(5)).unwrap();
        assert_eq!(String::from_utf8_lossy(&said.stdout), "hello");
    }

    #[test]
    fn nothing_from_what_is_not_there() {
        assert_eq!(text("no-such-program-here", &[], Duration::from_secs(5)), "");
    }

    #[test]
    fn what_hangs_is_stopped_with_what_it_started() {
        let at = Instant::now();
        // (`sleep` keeps the output open after its shell has gone.)
        assert_eq!(text("sh", &["-c", "sleep 30 & sleep 30"], Duration::from_millis(200)), "");
        assert!(at.elapsed() < Duration::from_secs(2));
    }
}
