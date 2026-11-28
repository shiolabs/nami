//! Sharing /nix/store with an installed Nix.
//!
//! When Nix is on the machine, its store belongs to it: nami never writes to
//! /nix/store itself. Downloads go through `nix-store --realise` (Nix checks
//! the signatures, with its own trusted keys), and what nami builds or adds
//! goes in through `nix-store --import`. Every path nami uses gets a GC root
//! in /nix/var/nix/gcroots/tray (its name from before nami's), so `nix-collect-garbage` leaves nami's
//! apps alone, and `nami gc` only deletes what Nix agrees is dead.
//!
//! The same paths, byte for byte: what Nix already has isn't downloaded
//! twice, and NixOS (read-only store) works too.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::store::{base_name, STORE_DIR};

const ROOTS: &str = "nix/var/nix/gcroots/tray"; // (Nix's, named as tray named it: the roots there stay nami's.)

pub struct Nix {
    bin: PathBuf,
    roots: PathBuf,
}

/// One store path as Nix records it.
pub struct Info {
    pub path: String,
    /// `sha256:<nix base32>`, as narinfos write it.
    pub nar_hash: String,
    pub nar_size: u64,
    /// Basenames.
    pub references: Vec<String>,
}

impl Nix {
    /// Nix, if it's installed under `root` (and `NAMI_NIX` isn't `off`).
    pub fn detect(root: &Path) -> Option<Nix> {
        if std::env::var("NAMI_NIX").is_ok_and(|v| v == "off") || !root.join("nix/var/nix/db/db.sqlite").exists() {
            return None;
        }
        let mut candidates: Vec<PathBuf> = ["nix/var/nix/profiles/default/bin", "run/current-system/sw/bin", "root/.nix-profile/bin"]
            .iter()
            .map(|d| root.join(d).join("nix-store"))
            .collect();
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|d| d.join("nix-store")));
        }
        let bin = candidates.into_iter().find(|c| c.exists())?;
        Some(Nix { bin, roots: root.join(ROOTS) })
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.bin);
        // Root talks to the store directly, which also works where /nix/store
        // is mounted read-only (NixOS): Nix remounts it for itself.
        c.env_remove("NIX_REMOTE");
        c
    }

    fn output(&self, mut c: Command, what: &str) -> Result<String> {
        let out = c.stderr(Stdio::piped()).output().with_context(|| format!("running {}", self.bin.display()))?;
        if !out.status.success() {
            bail!("nix-store {what} failed: {}", errors(&out.stderr));
        }
        Ok(String::from_utf8(out.stdout)?)
    }

    /// Download `paths` and their closures into Nix's store. `caches` are
    /// nami's extra binary caches (url, key), offered to Nix for this call.
    pub fn realise(&self, paths: &[String], caches: &[(String, String)]) -> Result<()> {
        let mut c = self.cmd();
        c.arg("--realise");
        if !caches.is_empty() {
            let urls: Vec<String> = caches.iter().map(|(u, _)| if u.starts_with('/') { format!("file://{u}") } else { u.clone() }).collect();
            let keys: Vec<&str> = caches.iter().map(|(_, k)| k.as_str()).collect();
            c.args(["--option", "extra-substituters", &urls.join(" ")]);
            c.args(["--option", "extra-trusted-public-keys", &keys.join(" ")]);
        }
        // No local builds: what the caches lack, nami builds itself.
        c.args(["--option", "max-jobs", "0"]);
        c.args(paths).stdout(Stdio::null());
        self.output(c, "--realise").map(drop)
    }

    /// The closure of `paths`, all of which must be valid.
    pub fn requisites(&self, paths: &[String]) -> Result<Vec<String>> {
        let mut c = self.cmd();
        c.args(["--query", "--requisites"]).args(paths);
        Ok(self.output(c, "--query")?.lines().map(str::to_string).collect())
    }

    /// Which of `paths` Nix has.
    pub fn valid(&self, paths: &[String]) -> Result<BTreeSet<String>> {
        if paths.is_empty() {
            return Ok(BTreeSet::new());
        }
        let mut c = self.cmd();
        c.args(["--check-validity", "--print-invalid"]).args(paths);
        let invalid: BTreeSet<String> = self.output(c, "--check-validity")?.lines().map(str::to_string).collect();
        Ok(paths.iter().filter(|p| !invalid.contains(*p)).cloned().collect())
    }

    /// Hash, size and references of valid `paths`.
    pub fn info(&self, paths: &[String]) -> Result<Vec<Info>> {
        let mut out = Vec::new();
        for chunk in paths.chunks(500) {
            let mut c = self.cmd();
            c.arg("--dump-db").args(chunk);
            out.extend(parse_dump_db(&self.output(c, "--dump-db")?)?);
        }
        Ok(out)
    }

    /// Hand finished trees to Nix: (store path, directory holding its
    /// contents, references as basenames). Nix hashes and registers them.
    pub fn import(&self, items: &[(String, PathBuf, Vec<String>)]) -> Result<()> {
        let mut child = self
            .cmd()
            .arg("--import")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("running {}", self.bin.display()))?;
        let mut w = std::io::BufWriter::new(child.stdin.take().unwrap());
        let written = (|| -> Result<()> {
            for (path, dir, refs) in items {
                export_one(&mut w, path, dir, refs)?;
            }
            num(&mut w, 0)?;
            w.flush()?;
            Ok(())
        })();
        drop(w);
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("nix-store --import failed: {}", errors(&out.stderr));
        }
        written
    }

    /// Keep `base` (and its closure) from Nix's garbage collector.
    pub fn add_root(&self, base: &str) -> Result<()> {
        fs::create_dir_all(&self.roots)?;
        let link = self.roots.join(base);
        if link.symlink_metadata().is_err() {
            std::os::unix::fs::symlink(format!("{STORE_DIR}/{base}"), &link)?;
        }
        Ok(())
    }

    /// Make nami's GC roots exactly `live`.
    pub fn sync_roots(&self, live: &BTreeSet<String>) -> Result<()> {
        fs::create_dir_all(&self.roots)?;
        for entry in fs::read_dir(&self.roots)? {
            let entry = entry?;
            if !live.contains(entry.file_name().to_string_lossy().as_ref()) {
                fs::remove_file(entry.path())?;
            }
        }
        live.iter().try_for_each(|b| self.add_root(b))
    }

    /// Delete those of `paths` nothing else needs (Nix refuses the rest).
    pub fn delete(&self, paths: &[String]) -> Result<()> {
        let mut todo: Vec<String> = paths.to_vec();
        // Nix stops at the first path still in use; drop it and go again.
        while !todo.is_empty() {
            let mut c = self.cmd();
            c.arg("--delete").args(&todo).stdout(Stdio::null());
            let out = c.stderr(Stdio::piped()).output()?;
            if out.status.success() {
                break;
            }
            let err = String::from_utf8_lossy(&out.stderr);
            let alive: Vec<String> = todo.iter().filter(|p| err.contains(p.as_str())).cloned().collect();
            if alive.is_empty() {
                bail!("nix-store --delete failed: {}", err.trim());
            }
            todo.retain(|p| !alive.contains(p));
            let still = self.valid(&todo)?;
            todo.retain(|p| still.contains(p));
        }
        Ok(())
    }
}

/// Nix's `error:` lines (without its retry warnings), or everything.
fn errors(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let errs: Vec<&str> = text.lines().filter(|l| l.trim_start().starts_with("error")).collect();
    if errs.is_empty() { text.trim().to_string() } else { errs.join("\n") }
}

/// `nix-store --dump-db` output: per path, its path, hex sha256, size,
/// deriver, number of references, references.
fn parse_dump_db(text: &str) -> Result<Vec<Info>> {
    let mut lines = text.lines();
    let mut out = Vec::new();
    while let Some(path) = lines.next() {
        if path.is_empty() {
            continue;
        }
        let mut next = || lines.next().with_context(|| format!("truncated --dump-db output at {path}"));
        let hex = next()?;
        let size: u64 = next()?.parse()?;
        let _deriver = next()?;
        let n: usize = next()?.parse()?;
        let mut references = Vec::with_capacity(n);
        for _ in 0..n {
            references.push(base_name(next()?).to_string());
        }
        let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)).collect::<Result<_, _>>()?;
        out.push(Info { path: path.to_string(), nar_hash: format!("sha256:{}", crate::nix::base32::encode(&bytes)), nar_size: size, references });
    }
    Ok(out)
}

fn num(w: &mut impl Write, n: u64) -> Result<()> {
    w.write_all(&n.to_le_bytes())?;
    Ok(())
}

fn string(w: &mut impl Write, s: &str) -> Result<()> {
    num(w, s.len() as u64)?;
    w.write_all(s.as_bytes())?;
    w.write_all(&[0u8; 8][..(8 - s.len() % 8) % 8])?;
    Ok(())
}

/// One path in Nix's export format: 1, the NAR, a magic number, the path,
/// its references, deriver (none) and signature flag (none).
fn export_one(w: &mut impl Write, path: &str, dir: &Path, refs: &[String]) -> Result<()> {
    num(w, 1)?;
    crate::nar::dump(dir, w)?;
    num(w, 0x4558494e)?;
    string(w, path)?;
    num(w, refs.len() as u64)?;
    for r in refs {
        string(w, &format!("{STORE_DIR}/{r}"))?;
    }
    string(w, "")?;
    num(w, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dump_db() {
        let text = "/nix/store/knhpzwcavw3d68lcq14f71d4ag8ykgz1-ripgrep-15.2.0\nee169197db4f12644cf7e0eb994b63295785fe27e3eceb8646287b07613cd3b7\n7335080\n/nix/store/9znzj51jnamrblnawph5kypfc8cd10d8-ripgrep-15.2.0.drv\n2\n/nix/store/hx9wvdx2l59iahwf9nmrx7rwkm6cymgs-glibc-2.44-25\n/nix/store/rcdkdkync4pgcdk38sxd6q7mkahp2861-pcre2-10.48\n";
        let i = &parse_dump_db(text).unwrap()[0];
        assert_eq!(i.nar_size, 7335080);
        assert_eq!(i.references, ["hx9wvdx2l59iahwf9nmrx7rwkm6cymgs-glibc-2.44-25", "rcdkdkync4pgcdk38sxd6q7mkahp2861-pcre2-10.48"]);
        assert!(i.nar_hash.starts_with("sha256:") && i.nar_hash.len() == 59);
    }
}
