//! `nami self-update`: new versions of nami itself, from the same place the
//! catalog comes from. `<base>/menu/<channel>` is where the catalog came
//! from, so `<base>/releases/latest` names the newest release.
//!
//! A release is trusted the way a catalog is: its SHA256SUMS is signed with a
//! catalog key this machine trusts (`SHA256SUMS.sig`), and the tarball must
//! match its line there. Then the release's own setup.sh installs it, exactly
//! as the installer would, keeping the apps (desktop, terminal) you chose. On
//! a computer nami made, the new nami simply replaces the old one.
//!
//! What's new in a release is its notes.md (docs/releases/<v>.md, which
//! packaging/publish.sh puts next to the tarball and lists in SHA256SUMS):
//! `--check --json` gives them, checked the same way, for Updates to show.
//! This nami's own are built in (build.rs), for after it's installed.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::Serialize;

use crate::catalog;
use crate::store::Store;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize)]
pub struct Check {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    /// Where releases come from: the server the catalog comes from (crate::site::SERVED_FROM unless the computer was set up from another).
    pub from: String,
    /// What's new in the latest: its notes (Markdown), when it has any.
    pub notes: Option<String>,
    /// The download's size in bytes, when there's a newer one.
    pub size: Option<u64>,
}

/// This nami's own release notes (docs/releases/<version>.md), if it has them.
pub const NOTES: Option<&str> = include!(concat!(env!("OUT_DIR"), "/notes.rs"));

/// The notes file in a release, as SHA256SUMS lists it.
pub const NOTES_FILE: &str = "notes.md";

/// The release server: the catalog's source without `/menu/<channel>`.
pub(crate) fn base(store: &Store) -> Result<String> {
    let source = fs::read_to_string(store.var_dir().join("index-source")).context("no catalog source recorded; run the installer, or `nami update --from <url>` first")?;
    let source = source.trim();
    ensure!(source.starts_with("https://") || source.starts_with("http://"), "the catalog comes from {source}, not a server, so there is no nami site to ask (for new versions of nami, or to send a report to)");
    let base = source.rsplit_once("/menu/").map(|(b, _)| b).ok_or_else(|| anyhow!("{source} isn't a <server>/menu/<channel> address"))?;
    Ok(base.to_string())
}

fn get(store: &Store, url: &str) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    match store.http().get(url).call() {
        Ok(r) => r.into_reader().read_to_end(&mut data).with_context(|| format!("downloading {url}"))?,
        Err(ureq::Error::Status(404, _)) => bail!("{url} doesn't exist"),
        Err(e) => return Err(e).with_context(|| format!("fetching {url}")),
    };
    Ok(data)
}

/// A release's tarball of nami for this computer: `nami-0.19.0-x86_64-linux.tar.gz`,
/// `nami-0.19.0-aarch64-darwin.tar.gz` on an Apple silicon Mac.
pub fn tarball(version: &str) -> String {
    format!("nami-{version}-{}.tar.gz", crate::index::SYSTEM)
}

/// `0.10.1` > `0.9.3`: numbers compared part by part.
fn newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| v.split(['.', '-']).map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parts(latest) > parts(current)
}

pub fn check(store: &Store) -> Result<Check> {
    let from = base(store)?;
    let latest = String::from_utf8(get(store, &crate::site::counted(store, &format!("{from}/releases/latest")))?)?.trim().to_string();
    ensure!(!latest.is_empty() && latest.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'), "{from}/releases/latest doesn't name a version");
    let newer = newer(&latest, CURRENT);
    // (What's new and how big: a release before 0.18.6 has no notes, and a
    // server that won't say a size needn't stop the check.)
    let (notes, size) = if newer {
        let rel = format!("{from}/releases/{latest}");
        (release_notes(store, &rel), size_of(store, &format!("{rel}/{}", tarball(&latest))))
    } else {
        (NOTES.map(String::from).filter(|_| latest == CURRENT), None)
    };
    Ok(Check { newer, current: CURRENT.into(), latest, from, notes, size })
}

/// A release's notes, if its signed SHA256SUMS lists them and they match.
fn release_notes(store: &Store, rel: &str) -> Option<String> {
    let sums = get(store, &format!("{rel}/SHA256SUMS")).ok()?;
    let sig = get(store, &format!("{rel}/SHA256SUMS.sig")).ok()?;
    catalog::verify(&sums, &String::from_utf8_lossy(&sig), &catalog::trusted_keys(store)).ok()?;
    let notes = get(store, &format!("{rel}/{NOTES_FILE}")).ok()?;
    checked_notes(&String::from_utf8_lossy(&sums), &notes)
}

/// `notes` as text, when `sums` (a release's SHA256SUMS) lists them with their hash.
fn checked_notes(sums: &str, notes: &[u8]) -> Option<String> {
    let want = sums.lines().find_map(|l| l.split_once("  ").filter(|(_, f)| *f == NOTES_FILE).map(|(h, _)| h))?;
    (catalog::sha256_hex(notes) == want).then(|| String::from_utf8_lossy(notes).trim().to_string()).filter(|n| !n.is_empty())
}

/// A download's size, as the server says it.
fn size_of(store: &Store, url: &str) -> Option<u64> {
    store.http().head(url).call().ok()?.header("content-length")?.parse().ok()
}

/// Download, check and install the newest release. Needs root.
pub fn run(store: &Store) -> Result<()> {
    let c = check(store)?;
    if !c.newer {
        crate::say!("✓ nami {} is the newest version", c.current);
        return Ok(());
    }
    ensure!(crate::profile::is_root(), "nami {} is out; installing it needs root: sudo nami self-update", c.latest);
    let rel = format!("{}/releases/{}", c.from, c.latest);
    let file = tarball(&c.latest);

    let sums = get(store, &format!("{rel}/SHA256SUMS"))?;
    let sig = get(store, &format!("{rel}/SHA256SUMS.sig")).map_err(|_| anyhow!("nami {} isn't signed, so it can't be checked; install it with the installer instead", c.latest))?;
    let signer = catalog::verify(&sums, &String::from_utf8_lossy(&sig), &catalog::trusted_keys(store)).context("the release's checksums")?;
    let want = String::from_utf8_lossy(&sums)
        .lines()
        .find_map(|l| l.split_once("  ").filter(|(_, f)| *f == file).map(|(h, _)| h.to_string()))
        .ok_or_else(|| anyhow!("{file} isn't in the release's checksums"))?;
    let who = catalog::signer_words(&signer, &catalog::trusted_keys(store));
    crate::say!("Downloading nami {} · {}", c.latest, if who == "namios.dev" { "verified".to_string() } else { format!("signed by {who}") });
    // ?via=update: the site counts it as an update, not a new install
    // (site/worker.js, /privacy), and a test computer's as a test's
    // (site::counted); nothing else about the computer is sent.
    let tarball = get(store, &crate::site::counted(store, &format!("{rel}/{file}?via=update")))?;
    ensure!(catalog::sha256_hex(&tarball) == want, "{file} doesn't match its checksum; nothing was installed");

    let tmp = std::env::temp_dir().join(format!("nami-self-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    let result = install(store, &tmp, &file, &tarball, &c);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn install(store: &Store, tmp: &Path, file: &str, tarball: &[u8], c: &Check) -> Result<()> {
    fs::write(tmp.join(file), tarball)?;
    let ok = Command::new("tar").arg("-xzf").arg(tmp.join(file)).arg("-C").arg(tmp).status().context("running tar")?.success();
    ensure!(ok, "unpacking {file} failed");
    let dir = tmp.join(format!("nami-{}", c.latest));
    // A computer nami made has nothing around nami to set up: the new nami
    // takes its place (it starts the next time it's run).
    if crate::nami_os::here() {
        let bin = store.var_dir().join("bin/nami");
        let new = store.var_dir().join("bin/nami.new");
        fs::copy(dir.join("nami"), &new).context("copying the new nami")?;
        fs::set_permissions(&new, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        fs::rename(&new, &bin).context("putting the new nami in place")?;
        crate::say!("✓ Updated nami to {} · was {}", c.latest, c.current);
        return Ok(());
    }
    let source = fs::read_to_string(store.var_dir().join("index-source"))?;
    crate::say!("installing nami {}", c.latest);
    let ok = Command::new("sh")
        .arg("setup.sh")
        .current_dir(&dir)
        .env("NAMI_CHANNEL", source.trim())
        .status()
        .context("running the release's setup.sh")?
        .success();
    ensure!(ok, "installing nami {} didn't finish; run `sudo nami self-update` again to retry", c.latest);
    crate::say!("✓ Updated nami to {} · was {}", c.latest, c.current);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{checked_notes, newer, NOTES_FILE};

    #[test]
    fn compares_versions_by_number() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn notes_count_only_when_the_signed_sums_list_them() {
        let notes = b"- Updates in one place.\n";
        let hash = crate::catalog::sha256_hex(notes);
        let sums = format!("{}  nami-0.18.6-x86_64-linux.tar.gz\n{hash}  {NOTES_FILE}\n", "0".repeat(64));
        assert_eq!(checked_notes(&sums, notes).as_deref(), Some("- Updates in one place."));
        // Changed after they were signed, or not listed (a release before notes): none.
        assert_eq!(checked_notes(&sums, b"- Something else.\n"), None);
        assert_eq!(checked_notes(&format!("{}  nami-0.18.6-x86_64-linux.tar.gz\n", "0".repeat(64)), notes), None);
        assert_eq!(checked_notes(&format!("{}  {NOTES_FILE}\n", crate::catalog::sha256_hex(b"")), b""), None);
    }

    #[test]
    fn every_release_has_short_plain_notes() {
        // docs/releases/<v>.md: what Updates shows, a few lines a person reads.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/releases");
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            assert!(name.ends_with(".md") && name.trim_end_matches(".md").split('.').all(|p| p.parse::<u32>().is_ok()), "{name}: <version>.md");
            assert!(text.lines().count() <= 40, "{name}: keep it short");
            assert!(!text.contains("journalctl") && !text.contains('`'), "{name}: plain words, no commands");
        }
    }

    #[test]
    fn the_changelog_has_every_release() {
        // CHANGELOG.md: each release's heading links its notes, then their first line.
        let top = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let log = std::fs::read_to_string(top.join("CHANGELOG.md")).unwrap();
        for e in std::fs::read_dir(top.join("docs/releases")).unwrap().flatten() {
            let v = e.file_name().to_string_lossy().trim_end_matches(".md").to_string();
            let first = std::fs::read_to_string(e.path()).unwrap().lines().next().unwrap_or("").trim().to_string();
            let heading = format!("## [{v}](docs/releases/{v}.md)");
            let at = log.find(&heading).unwrap_or_else(|| panic!("CHANGELOG.md: no {heading}"));
            assert!(log[at..].lines().skip(1).find(|l| !l.trim().is_empty()) == Some(first.as_str()), "CHANGELOG.md: {v}'s line isn't its notes' first: {first}");
        }
    }
}
