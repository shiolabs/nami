//! `nami keygen` and `nami push`: share what you built.
//!
//! A pushed cache is a plain directory in Nix's binary cache format
//! (`<hash>.narinfo`, `nar/<filehash>.nar.zst`, `nix-cache-info`), signed with
//! your key. Serve it from any static web host, S3 bucket or shared folder;
//! machines that list it under `caches:` download instead of rebuilding. Real
//! Nix can use it too.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};

use crate::nar::{self, HashingWriter};
use crate::narinfo::NarInfo;
use crate::store::{Store, STORE_DIR};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Root's keys live with nami's state; a user's in their config directory.
fn keys_dir(store: &Store) -> PathBuf {
    if store.daemon().is_none() {
        return store.var_dir().join("keys");
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("nami/keys")
}

/// Create a signing key. Returns the public key (`name:base64`) to put in
/// other machines' `caches:`.
pub fn keygen(store: &Store, name: &str) -> Result<String> {
    if name.is_empty() || name.contains([':', '/']) {
        bail!("key name must be non-empty, without ':' or '/'");
    }
    let dir = keys_dir(store);
    fs::create_dir_all(&dir)?;
    let secret_path = dir.join(format!("{name}.secret"));
    if secret_path.exists() {
        bail!("key {name} already exists ({})", secret_path.display());
    }
    let mut seed = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut seed)?;
    let key = SigningKey::from_bytes(&seed);
    let public = format!("{name}:{}", B64.encode(key.verifying_key().to_bytes()));
    // Same formats as `nix key generate-secret` / `convert-secret-to-public`.
    let secret = format!("{name}:{}", B64.encode(key.to_keypair_bytes()));
    let mut f = File::create(&secret_path)?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(secret.as_bytes())?;
    fs::write(dir.join(format!("{name}.public")), &public)?;
    Ok(public)
}

pub fn load_key(store: &Store, name: Option<&str>) -> Result<(String, SigningKey)> {
    let dir = keys_dir(store);
    let file = match name {
        Some(n) => dir.join(format!("{n}.secret")),
        None => {
            let mut secrets: Vec<PathBuf> = fs::read_dir(&dir)
                .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "secret")).collect())
                .unwrap_or_default();
            match secrets.len() {
                1 => secrets.pop().unwrap(),
                0 => bail!("no signing key; create one with `nami keygen <name>`"),
                _ => bail!("several signing keys; choose one with --key"),
            }
        }
    };
    let text = fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let (name, b64) = text.trim().split_once(':').ok_or_else(|| anyhow!("bad key file {}", file.display()))?;
    let bytes: [u8; 64] = B64.decode(b64)?.try_into().map_err(|_| anyhow!("bad key length in {}", file.display()))?;
    Ok((name.to_string(), SigningKey::from_keypair_bytes(&bytes)?))
}

/// Upload everything this machine built in the closures of `roots` to the
/// cache directory `dest`. Paths downloaded from caches are skipped: other
/// machines get those from where this one did. Returns how many were added.
pub fn push(store: &Store, dest: &Path, key_name: Option<&str>, roots: &[String]) -> Result<usize> {
    let (name, key) = load_key(store, key_name)?;
    fs::create_dir_all(dest.join("nar"))?;
    let info_file = dest.join("nix-cache-info");
    if !info_file.exists() {
        fs::write(&info_file, format!("StoreDir: {STORE_DIR}\nWantMassQuery: 1\nPriority: 40\n"))?;
    }

    let mut pushed = 0;
    for base in store.closure(roots)? {
        let info = store.path_info(&base)?;
        let hash = &base[..32];
        let narinfo_path = dest.join(format!("{hash}.narinfo"));
        if !info.built || narinfo_path.exists() {
            continue;
        }
        // Compressed NAR, named by the hash of the compressed file (like Nix).
        let tmp = dest.join("nar").join(format!(".upload-{hash}"));
        let mut enc = zstd::stream::write::Encoder::new(HashingWriter::new(BufWriter::new(File::create(&tmp)?)), 19)?;
        nar::dump(&store.phys(&format!("{STORE_DIR}/{base}")), &mut enc)?;
        let mut file = enc.finish()?;
        file.flush()?;
        let file_size = file.count;
        let file_hash = file.finish();
        let file_name = format!("nar/{}.nar.zst", file_hash.trim_start_matches("sha256:"));
        fs::rename(&tmp, dest.join(&file_name))?;

        let mut narinfo = NarInfo {
            store_path: format!("{STORE_DIR}/{base}"),
            url: file_name,
            compression: "zstd".into(),
            nar_hash: info.nar_hash,
            nar_size: info.nar_size,
            references: info.references,
            sigs: Vec::new(),
            cache: String::new(),
        };
        let sig = key.sign(narinfo.fingerprint().as_bytes());
        narinfo.sigs.push(format!("{name}:{}", B64.encode(sig.to_bytes())));
        let text = format!(
            "StorePath: {}\nURL: {}\nCompression: zstd\nFileHash: {file_hash}\nFileSize: {file_size}\nNarHash: {}\nNarSize: {}\nReferences: {}\nSig: {}\n",
            narinfo.store_path, narinfo.url, narinfo.nar_hash, narinfo.nar_size, narinfo.references.join(" "), narinfo.sigs[0],
        );
        // narinfo last: a reader never sees it before its NAR exists.
        let tmp_info = narinfo_path.with_extension("tmp");
        fs::write(&tmp_info, text)?;
        fs::rename(&tmp_info, &narinfo_path)?;
        crate::say!("  pushed {base}");
        pushed += 1;
    }
    Ok(pushed)
}
