//! Signed catalogs. Whoever controls the catalog chooses what `nami install
//! firefox` means, so machines only accept one signed by a key they trust.
//! The signature covers the index file; the index carries the sha256 of
//! every build definition and app icon, so one signature vouches for those too.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, Signer, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::store::{write_atomic, Store};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_raw(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn message(data: &[u8]) -> Vec<u8> {
    format!("tray-catalog-v1:{}", sha256_hex(data)).into_bytes()
}

fn keys_file(store: &Store) -> PathBuf {
    store.var_dir().join("catalog-keys.json")
}

pub fn trusted_keys(store: &Store) -> Vec<String> {
    fs::read(keys_file(store)).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

/// Trust catalogs signed by `key` (`name:base64`, as `nami keygen` prints).
pub fn trust(store: &Store, key: &str) -> Result<()> {
    let (name, b64) = key.split_once(':').ok_or_else(|| anyhow!("a key looks like name:base64"))?;
    ensure!(!name.is_empty() && B64.decode(b64).map(|k| k.len() == 32).unwrap_or(false), "{key:?} isn't a public key from `nami keygen`");
    let mut keys = trusted_keys(store);
    if !keys.iter().any(|k| k == key) {
        keys.push(key.to_string());
    }
    write_atomic(&keys_file(store), &serde_json::to_vec_pretty(&keys)?)
}

/// The keys nami's own site signs with (packaging/keys), compared by the key
/// itself, never by name: anyone may call a key "tray-3".
const OWN_KEYS: &str = include_str!("../packaging/keys");

/// Who signed, as a person reads it: nami's own keys are "namios.dev" (the name
/// of a key means nothing to them); anyone else's key by its name, so they
/// know whose catalog it is.
pub fn signer_words(name: &str, trusted: &[String]) -> String {
    let own = trusted.iter().filter(|k| k.split_once(':').is_some_and(|(n, _)| n == name)).any(|k| {
        OWN_KEYS.lines().map(str::trim).any(|l| !l.starts_with('#') && l == k.as_str())
    });
    if own { "namios.dev".into() } else { name.to_string() }
}

/// Check `sig_text` (`name:base64sig`) over `data`; returns the signer's name.
pub fn verify(data: &[u8], sig_text: &str, trusted: &[String]) -> Result<String> {
    verify_sha256(&sha256_hex(data), sig_text, trusted)
}

/// `verify`, for data whose sha256 (hex) is known already.
pub fn verify_sha256(sha256: &str, sig_text: &str, trusted: &[String]) -> Result<String> {
    let (name, b64) = sig_text.trim().split_once(':').ok_or_else(|| anyhow!("malformed signature"))?;
    let sig: [u8; 64] = B64.decode(b64)?.try_into().map_err(|_| anyhow!("malformed signature"))?;
    let key = trusted
        .iter()
        .find_map(|k| k.split_once(':').filter(|(n, _)| *n == name).map(|(_, k)| k))
        .ok_or_else(|| anyhow!("signed by {name:?}, which this machine doesn't trust (add it with `nami update --trust {name}:<key>`)"))?;
    let key: [u8; 32] = B64.decode(key)?.try_into().map_err(|_| anyhow!("bad trusted key for {name}"))?;
    VerifyingKey::from_bytes(&key)?
        .verify_strict(format!("tray-catalog-v1:{sha256}").as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| anyhow!("the signature by {name:?} doesn't match: the catalog was changed after signing"))?;
    Ok(name.to_string())
}

/// Sign `path` (an index file, or a catalog directory's index files).
pub fn sign(store: &Store, path: &Path, key_name: Option<&str>) -> Result<Vec<PathBuf>> {
    let (name, key) = crate::push::load_key(store, key_name)?;
    // A folder: its index-*.json(.xz); or, for a catalog's version
    // histories (versions/), every shard in it.
    let histories = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == "versions" || n.starts_with("versions-"));
    let files: Vec<PathBuf> = if path.is_dir() {
        fs::read_dir(path)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    if histories { n.ends_with(".json") } else { n.starts_with("index-") && (n.ends_with(".json") || n.ends_with(".json.xz")) }
                })
            })
            .collect()
    } else {
        vec![path.to_path_buf()]
    };
    if files.is_empty() {
        bail!("no index-*.json(.xz) in {}", path.display());
    }
    for f in &files {
        let data = fs::read(f).with_context(|| format!("reading {}", f.display()))?;
        let sig = key.sign(&message(&data));
        fs::write(PathBuf::from(format!("{}.sig", f.display())), format!("{name}:{}\n", B64.encode(sig.to_bytes())))?;
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn own_keys_are_said_as_namios_dev_and_only_by_the_key() {
        let own = OWN_KEYS.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap().to_string();
        let name = own.split_once(':').unwrap().0.to_string();
        assert_eq!(super::signer_words(&name, &[own.clone()]), "namios.dev");
        // Someone else's key, given the same name: said by its name.
        let fake = format!("{name}:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        assert_eq!(super::signer_words(&name, &[fake]), name);
        assert_eq!(super::signer_words("acme-1", &["acme-1:xyz".into()]), "acme-1");
    }

    #[test]
    fn sign_and_verify() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = format!("cat-1:{}", B64.encode(key.verifying_key().to_bytes()));
        let data = b"{\"jq\":{}}";
        let sig = format!("cat-1:{}", B64.encode(key.sign(&message(data)).to_bytes()));
        assert_eq!(verify(data, &sig, &[public.clone()]).unwrap(), "cat-1");
        assert!(verify(b"{\"jq\":{\"evil\":1}}", &sig, &[public]).is_err());
        assert!(verify(data, &sig, &[]).unwrap_err().to_string().contains("doesn't trust"));
    }

    // The web installer checks a release's signature with openssl, building the
    // same message by hand: a different context there refuses every release.
    #[test]
    fn web_installer_signs_the_same_message() {
        let installer = include_str!("../packaging/install-web.sh");
        let ours = String::from_utf8(message(b"")).unwrap();
        let context = ours.split(':').next().unwrap();
        assert!(installer.contains(&format!("printf '{context}:%s'")), "install-web.sh doesn't check `{context}:`");
    }
}
