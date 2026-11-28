//! A thin bundle's parts, fetched when it first starts: only in
//! nami-bundle-runtime-net. Each from a binary cache the bundle names, whose
//! narinfo must be signed with the key the bundle names for it and must
//! say the very NAR hash the bundle has for the part; then the NAR itself
//! is checked against that hash as it's unpacked. The bundle's own hash
//! covers its description, so nothing unchecked is ever used.

use std::io::{self, Read};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};

use crate::bundle_format::{self, Meta, PathEntry, Substituter};

pub fn fetch(meta: &Meta, p: &PathEntry, dest: &Path, from: &[u8], to: &[u8]) -> Result<()> {
    ensure!(!p.nar_hash.is_empty() && p.name.len() > 32, "{}: the bundle doesn't say what it is", p.name);
    ensure!(!meta.substituters.is_empty(), "{} isn't in the bundle, and it names nowhere to fetch it from", p.name);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("nami-bundle/", env!("CARGO_PKG_VERSION")))
        .build();
    let mut errors = Vec::new();
    for s in &meta.substituters {
        // A network that fails for a moment (a name not found, a reset)
        // is tried again, twice; a cache that says something wrong isn't.
        for attempt in 0..3 {
            match from_one(&agent, s, p, dest, from, to) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if dest.symlink_metadata().is_ok() {
                        let _ = std::fs::remove_dir_all(dest).or_else(|_| std::fs::remove_file(dest));
                    }
                    if attempt < 2 && transient(&e) {
                        std::thread::sleep(Duration::from_secs(1 + 2 * attempt));
                        continue;
                    }
                    errors.push(format!("{}: {e:#}", s.url));
                    break;
                }
            }
        }
    }
    bail!("fetching {}: {}", p.name, errors.join("; "))
}

/// The network's fault, for a moment: no connection, a name that didn't
/// resolve, a server busy or down (5xx, 429), a download cut off.
fn transient(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        if let Some(u) = c.downcast_ref::<ureq::Error>() {
            return match u {
                ureq::Error::Transport(_) => true,
                ureq::Error::Status(code, _) => *code >= 500 || *code == 429,
            };
        }
        c.downcast_ref::<io::Error>().is_some_and(|i| {
            matches!(i.kind(), io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe)
        })
    })
}

fn from_one(agent: &ureq::Agent, s: &Substituter, p: &PathEntry, dest: &Path, from: &[u8], to: &[u8]) -> Result<()> {
    let url = s.url.trim_end_matches('/');
    let text = agent.get(&format!("{url}/{}.narinfo", &p.name[..32])).call().context("its narinfo")?.into_string()?;
    let info = crate::narinfo::NarInfo::parse(&text)?;
    ensure!(info.base_name() == p.name, "the cache describes {} instead", info.store_path);
    info.verify(&[s.key.as_str()]).context("its narinfo isn't signed by the key the bundle names")?;
    ensure!(info.nar_hash == p.nar_hash, "the cache's is another build ({}, not {})", info.nar_hash, p.nar_hash);
    let body = agent.get(&format!("{url}/{}", info.url)).call().context("its NAR")?.into_reader();
    let decompressed: Box<dyn Read> = match info.compression.as_str() {
        "xz" => Box::new(xz2::read::XzDecoder::new(body)),
        "zstd" => Box::new(zstd::stream::read::Decoder::new(body)?),
        "none" => Box::new(body),
        other => bail!("{other} compression isn't something this runtime reads"),
    };
    let mut hashing = Hashing { inner: decompressed, hash: Sha256::new(), count: 0 };
    bundle_format::unpack(&mut hashing, dest, from, to)?;
    io::copy(&mut hashing, &mut io::sink())?;
    let got = format!("sha256:{}", crate::nix::base32::encode(&hashing.hash.finalize()));
    ensure!(got == p.nar_hash && hashing.count == p.nar_size, "what came isn't what the bundle says ({got})");
    Ok(())
}

struct Hashing<R> {
    inner: R,
    hash: Sha256,
    count: u64,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        self.count += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_again_only_what_the_network_did() {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).build();
        // Nothing listens there: no connection.
        let refused = agent.get("http://127.0.0.1:1/x.narinfo").call().map(|_| ()).context("its narinfo").unwrap_err();
        assert!(transient(&refused), "{refused:#}");
        let busy = ureq::Error::Status(503, ureq::Response::new(503, "Service Unavailable", "").unwrap());
        assert!(transient(&anyhow::Error::new(busy).context("its narinfo")));
        let gone = ureq::Error::Status(404, ureq::Response::new(404, "Not Found", "").unwrap());
        assert!(!transient(&anyhow::Error::new(gone).context("its narinfo")));
        let cut = io::Error::new(io::ErrorKind::UnexpectedEof, "cut off");
        assert!(transient(&anyhow::Error::new(cut).context("its NAR")));
        // What the cache says, checked and wrong: not the network's.
        assert!(!transient(&anyhow::anyhow!("the cache's is another build")));
    }
}
