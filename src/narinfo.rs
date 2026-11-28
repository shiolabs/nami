//! `.narinfo` files: metadata for one store path in a binary cache.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};

use crate::store::STORE_DIR;

#[derive(Debug, Clone)]
pub struct NarInfo {
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub nar_hash: String,
    pub nar_size: u64,
    /// Store path basenames (`<hash>-<name>`), possibly including itself.
    pub references: Vec<String>,
    pub sigs: Vec<String>,
    /// The cache this came from (not part of the file).
    pub cache: String,
}

impl NarInfo {
    pub fn parse(text: &str) -> Result<Self> {
        let (mut store_path, mut url, mut nar_hash, mut nar_size) = (None, None, None, None);
        let mut compression = "bzip2".to_string(); // Nix's historical default
        let (mut references, mut sigs) = (Vec::new(), Vec::new());
        for line in text.lines() {
            let Some((key, value)) = line.split_once(": ") else { continue };
            match key {
                "StorePath" => store_path = Some(value.to_string()),
                "URL" => url = Some(value.to_string()),
                "Compression" => compression = value.to_string(),
                "NarHash" => nar_hash = Some(value.to_string()),
                "NarSize" => nar_size = Some(value.parse().context("bad NarSize")?),
                "References" => references = value.split_whitespace().map(String::from).collect(),
                "Sig" => sigs.push(value.to_string()),
                _ => {}
            }
        }
        Ok(NarInfo {
            store_path: store_path.ok_or_else(|| anyhow!("narinfo missing StorePath"))?,
            url: url.ok_or_else(|| anyhow!("narinfo missing URL"))?,
            compression,
            nar_hash: nar_hash.ok_or_else(|| anyhow!("narinfo missing NarHash"))?,
            nar_size: nar_size.ok_or_else(|| anyhow!("narinfo missing NarSize"))?,
            references,
            sigs,
            cache: String::new(),
        })
    }

    /// The string a cache signs: `1;<path>;<narHash>;<narSize>;<ref,ref,...>`.
    pub fn fingerprint(&self) -> String {
        let refs: Vec<String> = self.references.iter().map(|r| format!("{STORE_DIR}/{r}")).collect();
        format!("1;{};{};{};{}", self.store_path, self.nar_hash, self.nar_size, refs.join(","))
    }

    /// Succeeds if at least one signature is valid under one of `trusted` (`name:base64key`).
    pub fn verify(&self, trusted: &[&str]) -> Result<()> {
        let fingerprint = self.fingerprint();
        for sig in &self.sigs {
            let Some((sig_name, sig_b64)) = sig.split_once(':') else { continue };
            for key in trusted {
                let Some((key_name, key_b64)) = key.split_once(':') else { continue };
                if key_name == sig_name && check(key_b64, sig_b64, fingerprint.as_bytes()).is_ok() {
                    return Ok(());
                }
            }
        }
        bail!("no valid trusted signature for {}", self.store_path)
    }

    pub fn base_name(&self) -> &str {
        self.store_path.trim_start_matches(STORE_DIR).trim_start_matches('/')
    }
}

fn check(key_b64: &str, sig_b64: &str, msg: &[u8]) -> Result<()> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = b64.decode(key_b64)?.try_into().map_err(|_| anyhow!("bad key length"))?;
    let sig: [u8; 64] = b64.decode(sig_b64)?.try_into().map_err(|_| anyhow!("bad sig length"))?;
    VerifyingKey::from_bytes(&key)?.verify_strict(msg, &Signature::from_bytes(&sig))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Real narinfo from cache.nixos.org (nixos-unstable, 2026-10).
    pub(crate) const RIPGREP: &str = "StorePath: /nix/store/knhpzwcavw3d68lcq14f71d4ag8ykgz1-ripgrep-15.2.0
URL: nar/1dyk7ihhfyr88s3fpv734zz8amr9cd5rksz0yx6684jgvfbr25pf.nar.zst
Compression: zstd
FileHash: sha256:0xkmdd9bizjzav9bg3lqnr1y9j9zax2p2gqbc3lgc2wybkvrp9hj
FileSize: 2076137
NarHash: sha256:1dyk7ihhfyr88s3fpv734zz8amr9cd5rksz0yx6684jgvfbr25pf
NarSize: 7335080
References: hx9wvdx2l59iahwf9nmrx7rwkm6cymgs-glibc-2.44-25 na9inwswsrgxxqpqrz3ms78qn7pyxg44-gcc-16.2.0-lib rcdkdkync4pgcdk38sxd6q7mkahp2861-pcre2-10.48
Deriver: 9znzj51jnamrblnawph5kypfc8cd10d8-ripgrep-15.2.0.drv
Sig: cache.nixos.org-1:MJn2RK4U9p9kentu8vqLg9MBxwB+5jt0IZhm3j/jeqZzCKZCPPX+X7Vk85MJWzBrAVT2FjTB/tKp8m80fZVQDw==
";

    #[test]
    fn verifies_real_signature() {
        let info = NarInfo::parse(RIPGREP).unwrap();
        info.verify(&[crate::store::NIXOS_CACHE_KEY]).unwrap();
    }

    #[test]
    fn rejects_tampered_references() {
        let mut info = NarInfo::parse(RIPGREP).unwrap();
        info.references.pop();
        assert!(info.verify(&[crate::store::NIXOS_CACHE_KEY]).is_err());
    }
}
