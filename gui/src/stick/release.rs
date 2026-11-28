//! The namiOS release to write: the version the site says is the latest,
//! and its image's address, size and checksum, from the release's
//! SHA256SUMS, checked against its signature (`nami catalog sign`'s, the
//! same that `nami self-update` trusts) with the key built in.

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// Where releases are (NAMI_STICK_BASE for another, as for a test).
pub fn base() -> String {
    std::env::var("NAMI_STICK_BASE").unwrap_or_else(|_| option_env!("NAMI_STICK_BASE").unwrap_or("https://namios.dev").to_string())
}

/// The keys nami's site is signed with (the installer's, packaging/keys).
const KEYS: &str = include_str!("../../../packaging/keys");

/// The keys a release's SHA256SUMS may be signed with, `name:base64`
/// (NAMI_STICK_KEY for another, as for a test; empty: none).
fn keys() -> Option<Vec<String>> {
    let keys = match std::env::var("NAMI_STICK_KEY").ok().or_else(|| option_env!("NAMI_STICK_KEY").map(String::from)) {
        Some(k) => vec![k],
        None => KEYS.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect(),
    };
    Some(keys.into_iter().filter(|k| !k.is_empty()).collect::<Vec<_>>()).filter(|k| !k.is_empty())
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// The download's size (compressed), in bytes.
    pub size: u64,
    /// Its SHA-256, as SHA256SUMS says.
    pub sha256: String,
}

fn get(url: &str) -> Result<String, String> {
    let r = ureq::get(url).timeout(std::time::Duration::from_secs(30)).call().map_err(|e| format!("{url}: {e}"))?;
    r.into_string().map_err(|e| format!("{url}: {e}"))
}

/// The latest release with a namiOS image.
pub fn latest() -> Result<Release, String> {
    let (version, url, sha256) = choose(&base(), &get, keys())?;
    let head = ureq::head(&url).timeout(std::time::Duration::from_secs(30)).call().map_err(|e| format!("{url}: {e}"))?;
    let size = head.header("content-length").and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok(Release { version, url, size, sha256 })
}

/// A version as releases/ names it, and nothing that leads elsewhere.
fn plain(v: &str) -> Option<&str> {
    let v = v.trim();
    let ok = !v.is_empty() && v.len() <= 64 && !v.starts_with('.') && v.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    ok.then_some(v)
}

/// The release whose image to write, the image's address and its SHA-256:
/// the release releases/latest-image names (the newest with an image; not
/// every release has one), else, on a server from before that file,
/// releases/latest. Either way, checked against that release's signed
/// SHA256SUMS.
fn choose(base: &str, get: &dyn Fn(&str) -> Result<String, String>, keys: Option<Vec<String>>) -> Result<(String, String, String), String> {
    let version = match get(&format!("{base}/releases/latest-image")).ok().as_deref().and_then(plain) {
        Some(v) => v.to_string(),
        None => {
            let latest = get(&format!("{base}/releases/latest"))?;
            plain(&latest).ok_or_else(|| format!("{base}/releases/latest doesn't name a release"))?.to_string()
        }
    };
    let rel = format!("{base}/releases/{version}");
    let sums = get(&format!("{rel}/SHA256SUMS"))?;
    match keys {
        Some(keys) => {
            let sig = get(&format!("{rel}/SHA256SUMS.sig")).map_err(|_| format!("namiOS {version} isn't signed, so it can't be checked"))?;
            // (The key it names, of the ones built in.)
            let name = sig.trim().split_once(':').map(|(n, _)| n).unwrap_or_default();
            let k = keys.iter().find(|k| k.split_once(':').is_some_and(|(kn, _)| kn == name)).unwrap_or(&keys[0]);
            verify(sums.as_bytes(), &sig, k)?;
        }
        // A build without the key (a test's): HTTPS alone.
        None => eprintln!("namios-installer: no key built in: SHA256SUMS isn't checked"),
    }
    let file = format!("namios-{version}-x86_64.img.xz");
    let sha256 = sums
        .lines()
        .find_map(|l| l.split_once("  ").filter(|(_, f)| *f == file).map(|(h, _)| h.to_string()))
        .ok_or_else(|| format!("namiOS {version} has no image yet ({file})"))?;
    Ok((version, format!("{rel}/{file}"), sha256))
}

/// `sig_text` (`name:base64`) is `key`'s signature of `data`, as
/// `nami catalog sign` makes it.
pub fn verify(data: &[u8], sig_text: &str, key: &str) -> Result<(), String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let bad = || "the release's signature doesn't match: don't use this download".to_string();
    let (name, sig) = sig_text.trim().split_once(':').ok_or_else(bad)?;
    let (kname, k) = key.split_once(':').ok_or("a bad key built in")?;
    if name != kname {
        return Err(format!("the release is signed by {name}, not {kname}"));
    }
    let sig: [u8; 64] = b64.decode(sig).ok().and_then(|s| s.try_into().ok()).ok_or_else(bad)?;
    let k: [u8; 32] = b64.decode(k).ok().and_then(|s| s.try_into().ok()).ok_or("a bad key built in")?;
    let message = format!("tray-catalog-v1:{}", hex(&Sha256::digest(data)));
    VerifyingKey::from_bytes(&k).map_err(|_| "a bad key built in".to_string())?.verify_strict(message.as_bytes(), &Signature::from_bytes(&sig)).map_err(|_| bad())
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn the_sites_keys_are_built_in() {
        let keys = keys().unwrap();
        assert!(keys.iter().any(|k| k.starts_with("tray-2:")) && keys.iter().any(|k| k.starts_with("tray-3:")), "{keys:?}");
        assert!(keys.iter().all(|k| k.split_once(':').is_some_and(|(_, b)| b.len() == 44)));
    }

    /// A site in memory (path, text), at https://x.
    fn site(files: &[(&str, &str)]) -> Box<dyn Fn(&str) -> Result<String, String>> {
        let files: std::collections::HashMap<String, String> = files.iter().map(|(p, t)| (format!("https://x/{p}"), t.to_string())).collect();
        Box::new(move |url: &str| files.get(url).cloned().ok_or_else(|| format!("{url}: 404")))
    }

    /// A key, and its public half as the Installer has it built in.
    fn key() -> (SigningKey, String) {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let public = format!("t-1:{}", base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes()));
        (key, public)
    }

    /// `data` signed as `nami catalog sign` signs SHA256SUMS.
    fn sign(key: &SigningKey, data: &str) -> String {
        let message = format!("tray-catalog-v1:{}", hex(&Sha256::digest(data.as_bytes())));
        format!("t-1:{}", base64::engine::general_purpose::STANDARD.encode(key.sign(message.as_bytes()).to_bytes()))
    }

    #[test]
    fn the_newest_release_with_an_image_not_the_latest() {
        let (key, public) = key();
        let old = "aa  namios-0.6-x86_64.img.xz\nbb  nami-0.6-x86_64-linux.tar.gz\n";
        let new = "cc  nami-0.9-x86_64-linux.tar.gz\n";
        let (old_sig, new_sig) = (sign(&key, old), sign(&key, new));
        let get = site(&[
            ("releases/latest", "0.9\n"),
            ("releases/latest-image", "0.6\n"),
            ("releases/0.6/SHA256SUMS", old),
            ("releases/0.6/SHA256SUMS.sig", &old_sig),
            ("releases/0.9/SHA256SUMS", new),
            ("releases/0.9/SHA256SUMS.sig", &new_sig),
        ]);
        let (v, url, sha) = choose("https://x", &get, Some(vec![public])).unwrap();
        assert_eq!((v.as_str(), url.as_str(), sha.as_str()), ("0.6", "https://x/releases/0.6/namios-0.6-x86_64.img.xz", "aa"));
    }

    #[test]
    fn a_server_without_latest_image_has_latest_read() {
        let (key, public) = key();
        let with = "dd  namios-0.9-x86_64.img.xz\n";
        let without = "cc  nami-0.9-x86_64-linux.tar.gz\n";
        let get = site(&[("releases/latest", "0.9\n"), ("releases/0.9/SHA256SUMS", with), ("releases/0.9/SHA256SUMS.sig", &sign(&key, with))]);
        assert_eq!(choose("https://x", &get, Some(vec![public.clone()])).unwrap().0, "0.9");
        // Latest has no image, and nothing says which has: said so, as before.
        let get = site(&[("releases/latest", "0.9"), ("releases/0.9/SHA256SUMS", without), ("releases/0.9/SHA256SUMS.sig", &sign(&key, without))]);
        assert!(choose("https://x", &get, Some(vec![public.clone()])).unwrap_err().contains("namiOS 0.9 has no image yet"));
        // A latest-image that isn't a version is passed over.
        let get = site(&[
            ("releases/latest-image", "../evil"),
            ("releases/latest", "0.9"),
            ("releases/0.9/SHA256SUMS", with),
            ("releases/0.9/SHA256SUMS.sig", &sign(&key, with)),
        ]);
        assert_eq!(choose("https://x", &get, Some(vec![public])).unwrap().0, "0.9");
    }

    #[test]
    fn latest_images_release_is_checked_as_latest_was() {
        let (key, public) = key();
        let sums = "aa  namios-0.6-x86_64.img.xz\n";
        let forged = sign(&key, "evil  namios-0.6-x86_64.img.xz\n");
        let get = site(&[("releases/latest", "0.9"), ("releases/latest-image", "0.6"), ("releases/0.6/SHA256SUMS", sums), ("releases/0.6/SHA256SUMS.sig", &forged)]);
        assert!(choose("https://x", &get, Some(vec![public.clone()])).unwrap_err().contains("doesn't match"));
        let get = site(&[("releases/latest", "0.9"), ("releases/latest-image", "0.6"), ("releases/0.6/SHA256SUMS", sums)]);
        assert!(choose("https://x", &get, Some(vec![public])).unwrap_err().contains("isn't signed"));
    }

    #[test]
    fn the_releases_signature() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let public = format!("nami.sh-1:{}", b64.encode(key.verifying_key().to_bytes()));
        let sums = b"abc  namios-1-x86_64.img.xz\n";
        let message = format!("tray-catalog-v1:{}", hex(&Sha256::digest(sums)));
        let sig = format!("nami.sh-1:{}", b64.encode(key.sign(message.as_bytes()).to_bytes()));
        assert!(verify(sums, &sig, &public).is_ok());
        assert!(verify(b"evil  namios-1-x86_64.img.xz\n", &sig, &public).is_err());
        assert!(verify(sums, &sig.replace("nami.sh-1", "other"), &public).unwrap_err().contains("signed by other"));
    }
}
