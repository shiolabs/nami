//! How far a running nami command is, from what it says: nami says
//! `fetching 48 paths (210.4 MiB unpacked)`, then each path as it lands
//! (`  <hash>-libreoffice-26.2.3`), as many times as it fetches. Only
//! what's coming down is said ("Downloading 120 MB…"): no bar, no time left.

#[derive(Default, Debug, PartialEq)]
pub struct Progress {
    /// Paths to fetch, and their size unpacked (MiB), in all.
    pub total: usize,
    pub mib: f64,
    /// Paths fetched, and the last one's name (libreoffice-26.2.3).
    pub done: usize,
    pub now: Option<String>,
}

impl Progress {
    pub fn read(lines: &[String]) -> Self {
        let mut p = Progress::default();
        for l in lines {
            if let Some(rest) = l.strip_prefix("fetching ") {
                let mut words = rest.split_whitespace();
                if let (Some(n), Some("paths")) = (words.next().and_then(|n| n.parse::<usize>().ok()), words.next()) {
                    p.total += n;
                    p.mib += words.next().and_then(|m| m.trim_start_matches('(').parse::<f64>().ok()).unwrap_or(0.);
                }
            } else if let Some(name) = landed(l) {
                p.done += 1;
                p.now = Some(name.to_string());
            }
        }
        p.done = p.done.min(p.total);
        p
    }
}

/// A path that landed: `  <32 base32 characters>-name`, as its name.
pub fn landed(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("  ")?;
    let (hash, name) = rest.split_at_checked(32)?;
    let name = name.strip_prefix('-')?;
    (hash.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()) && !name.is_empty() && !name.contains(' ')).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn counts_what_landed() {
        let p = Progress::read(&lines(&[
            "resolving 1412 packages",
            "fetching 3 paths (42.5 MiB unpacked)",
            "  0c3ssd2rjqakfggl1xcgndrxq3ll8bqz-libreoffice-26.2.3",
            "  1d8kd9s0q7n3c2mqcz9wq9a1b6xv0jrf-vlc-3.0.24",
            "fetching 1 paths (1.0 MiB unpacked)",
        ]));
        assert_eq!(p, Progress { total: 4, mib: 43.5, done: 2, now: Some("vlc-3.0.24".into()) });
    }

    #[test]
    fn other_lines_are_not_paths() {
        let p = Progress::read(&lines(&["  ok   something", "fetching 2 paths through Nix", "  short-name"]));
        assert_eq!((p.total, p.done), (2, 0));
        assert_eq!(Progress::read(&lines(&["switched to system 3"])).total, 0);
    }

    #[test]
    fn says_what_comes_down() {
        let mut job = crate::jobs::Job::new_for_test();
        assert_eq!(job.note(), None);
        job.lines = lines(&["fetching 30 paths (114.6 MiB unpacked)", "  0c3ssd2rjqakfggl1xcgndrxq3ll8bqz-geogebra-6.0"]);
        assert_eq!(job.note().as_deref(), Some("Downloading 120 MB…"));
        job.lines = lines(&["fetching 300 paths (2470.0 MiB unpacked)"]);
        assert_eq!(job.note().as_deref(), Some("Downloading 2.6 GB…"));
        job.lines = lines(&["fetching 2 paths (1.2 MiB unpacked)"]);
        assert_eq!(job.note(), None);
    }
}
