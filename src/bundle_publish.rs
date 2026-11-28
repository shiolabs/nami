//! The developer's half of self-updating bundles (src/bundle_update.rs):
//! the signing key `nami bundle --update-url` makes and keeps, and
//! `--publish-dir`, which writes the feed and the parts to upload.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine;
use ed25519_dalek::SigningKey;

use crate::bundle_delta as delta;
use crate::bundle_format::{self as format, Update};
use crate::bundle_update::{self as update, Feed};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// The secret key in a CI's environment (the key file's contents).
pub const KEY_VAR: &str = "NAMI_BUNDLE_SIGNING_KEY";

/// Where the key is kept: `~/.config/nami/bundle-signing.key`.
pub fn key_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("nami/bundle-signing.key")
}

/// `name:base64(secret and public, 64 bytes)`, as `nami keygen` and Nix
/// write them, on the first line; after a `--rotate-key`, the handovers
/// from the keys before it, a line each (`rotation {"from": …}`).
fn parse(text: &str, from: &str) -> Result<(String, SigningKey)> {
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let (name, b64) = first.split_once(':').ok_or_else(|| anyhow!("{from} isn't a signing key (name:base64)"))?;
    let bytes: [u8; 64] = B64.decode(b64.trim()).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| anyhow!("{from} isn't a signing key (name:base64 of 64 bytes)"))?;
    Ok((name.to_string(), SigningKey::from_keypair_bytes(&bytes).map_err(|_| anyhow!("{from}: its two halves don't match"))?))
}

/// The handovers a key file carries.
fn rotations_in(text: &str) -> Result<Vec<format::Rotation>> {
    text.lines().filter_map(|l| l.trim().strip_prefix("rotation ")).map(|j| serde_json::from_str(j).context("a rotation in the signing key")).collect()
}

/// The key's handovers: NAMI_BUNDLE_SIGNING_KEY's, else the key file's.
pub fn rotations() -> Result<Vec<format::Rotation>> {
    match std::env::var(KEY_VAR).ok().filter(|t| !t.trim().is_empty()) {
        Some(text) => rotations_in(&text),
        None => rotations_in(&fs::read_to_string(key_file()).unwrap_or_default()),
    }
}

/// `nami bundle --rotate-key`: a new signing key, handed over to by the one
/// you have (it signs the handover), which is kept beside it. Your next
/// release carries the handover, and the copies people have follow it to
/// the new key by themselves; any other change of key they refuse.
pub fn rotate_key() -> Result<()> {
    ensure!(std::env::var(KEY_VAR).ok().filter(|t| !t.trim().is_empty()).is_none(), "rotate your key on your computer (where {} is), then put the new file's contents in {KEY_VAR}", key_file().display());
    let file = key_file();
    let text = fs::read_to_string(&file).with_context(|| format!("no signing key at {} to hand over from", file.display()))?;
    let (old_name, old) = parse(&text, &file.display().to_string())?;
    let mut chain = rotations_in(&text)?;
    let mut seed = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut seed)?;
    let key = SigningKey::from_bytes(&seed);
    let name = format!("bundle-{}", &format::sha256_hex(key.verifying_key().as_bytes())[..8]);
    let (from, to) = (update::public_key(&old_name, &old), update::public_key(&name, &key));
    chain.push(update::sign_rotation(&old, &from, &to));
    // The old key kept, in case (its handover is signed already).
    let kept = file.with_file_name(format!("bundle-signing-{old_name}.key"));
    fs::copy(&file, &kept)?;
    fs::set_permissions(&kept, fs::Permissions::from_mode(0o600))?;
    let mut out = format!("{name}:{}\n", B64.encode(key.to_keypair_bytes()));
    for r in &chain {
        out += &format!("rotation {}\n", serde_json::to_string(r)?);
    }
    let tmp = file.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(out.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, &file)?;
    println!("{to}");
    eprintln!(
        "fingerprint: {} (was {})\nYour next release (--publish-dir) is signed with it and carries the handover: the copies people have move to it by themselves.\nThe old key is kept in {}. For CI, put the new contents of {} in {KEY_VAR}.",
        update::fingerprint(&to).unwrap_or_default(),
        update::fingerprint(&from).unwrap_or_default(),
        kept.display(),
        file.display()
    );
    Ok(())
}

/// The developer's signing key: NAMI_BUNDLE_SIGNING_KEY's (in CI), else the
/// one kept in ~/.config/nami, made the first time.
pub fn signing_key() -> Result<(String, SigningKey)> {
    if let Some(text) = std::env::var(KEY_VAR).ok().filter(|t| !t.trim().is_empty()) {
        return parse(&text, KEY_VAR);
    }
    let file = key_file();
    if let Ok(text) = fs::read_to_string(&file) {
        return parse(&text, &file.display().to_string());
    }
    // A CI machine starts empty: a key made there would sign updates no one's
    // copy accepts, and be gone after the run.
    if std::env::var_os("CI").is_some_and(|v| !v.is_empty()) {
        bail!("no signing key: on CI, put the contents of {} (from your computer) in a secret, and give it to nami as {KEY_VAR}", file.display());
    }
    let mut seed = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut seed)?;
    let key = SigningKey::from_bytes(&seed);
    let name = format!("bundle-{}", &format::sha256_hex(key.verifying_key().as_bytes())[..8]);
    fs::create_dir_all(file.parent().unwrap())?;
    let tmp = file.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(format!("{name}:{}\n", B64.encode(key.to_keypair_bytes())).as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, &file)?;
    crate::log::settle();
    let said = format!(
        "✓ Made your bundle signing key · {} · {}\n  Keep it secret, and back it up: your bundles take updates signed with it only.\n  For CI, put its contents in a secret named {KEY_VAR}.",
        file.display(),
        update::public_key(&name, &key)
    );
    eprintln!("{}", crate::style::err().lines(&said, crate::style::Paint::line));
    Ok((name, key))
}

/// What goes in a bundle made with --update-url.
pub fn update_info(url: &str) -> Result<Update> {
    let feed = update::feed_url(url);
    ensure!(feed.starts_with("https://") || feed.starts_with("http://"), "--update-url {url}: an https:// URL (the folder you publish to), or github:<owner>/<repo>");
    let (name, key) = signing_key()?;
    // Builds made from one commit (SOURCE_DATE_EPOCH) count as made then.
    let ms = match std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(s) => s * 1000,
        None => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64,
    };
    Ok(Update { feed, key: update::public_key(&name, &key), serial: ms, rotations: rotations()? })
}

/// `nami bundle --show-signing-key`.
pub fn show_key() -> Result<()> {
    let from_env = std::env::var(KEY_VAR).ok().filter(|t| !t.trim().is_empty()).is_some();
    let (name, key) = signing_key()?;
    let public = update::public_key(&name, &key);
    println!("{public}");
    // (What namiOS's Settings shows people who install your bundles.)
    eprintln!("fingerprint: {}", update::fingerprint(&public).unwrap_or_default());
    if from_env {
        eprintln!("(from {KEY_VAR})");
    } else {
        eprintln!("kept in {}: for CI, put its contents in a secret named {KEY_VAR}", key_file().display());
    }
    Ok(())
}

/// Write the feed and the parts of `bundle` (made with --update-url, its
/// frames at `level`) to `dir`, every file at the top: upload them all to
/// the --update-url folder. With patches from the release before
/// (`previous`, src/bundle_delta.rs), for the copies people have.
pub fn publish(bundle: &Path, dir: &Path, level: i32, previous: Option<&str>) -> Result<Vec<PathBuf>> {
    let mut f = File::open(bundle).with_context(|| format!("opening {}", bundle.display()))?;
    let (trailer, meta) = format::read(&mut f)?;
    let u = meta.update.clone().context("the bundle doesn't update itself: make it with --update-url")?;
    let (name, key) = signing_key()?;
    ensure!(update::public_key(&name, &key) == u.key, "the bundle names another key than yours ({})", u.key);
    fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
    // The release before, before this one's feed takes its place.
    let previous = Previous::find(previous, dir, &u.feed)?;
    let mut written = Vec::new();
    let mut copy = |offset: u64, size: u64, out: &str| -> Result<()> {
        let dest = dir.join(out);
        if dest.metadata().is_ok_and(|m| m.len() == size) {
            return Ok(());
        }
        f.seek(SeekFrom::Start(offset))?;
        let tmp = dir.join(format!(".{out}.tmp"));
        std::io::copy(&mut (&f).take(size), &mut File::create(&tmp)?)?;
        fs::rename(&tmp, &dest)?;
        written.push(dest);
        Ok(())
    };
    copy(0, trailer.payload_offset, &update::runtime_file(&meta.runtime))?;
    for p in meta.paths.iter().filter(|p| !p.fetch) {
        copy(p.offset, p.size, &update::part_file(&p.sha256))?;
    }
    copy(trailer.meta_offset, trailer.meta_len, &update::meta_file(&trailer.hash()))?;
    let file = bundle.file_name().context("the bundle's file name")?.to_string_lossy().into_owned();
    let size = fs::metadata(bundle)?.len();
    let whole = dir.join(&file);
    if fs::canonicalize(&whole).ok() != fs::canonicalize(bundle).ok() {
        let tmp = dir.join(format!(".{file}.tmp"));
        fs::copy(bundle, &tmp)?;
        fs::rename(&tmp, &whole)?;
    }
    let mut feed = Feed { format: 1, name: meta.name.clone(), version: meta.version.clone(), serial: u.serial, sha256: trailer.hash(), size, file, signature: String::new(), rotations: u.rotations.clone(), ..Default::default() };
    feed.sign(&name, &key);
    feed.verify(&u.key)?;
    if let Some(prev) = &previous {
        let (frames, patches) = if prev.hash == feed.sha256 {
            // This release again: the patches it has stay.
            (prev.feed.frames.clone(), prev.feed.patches.clone())
        } else {
            (Some(delta::Frames::here(level)), make_patches(bundle, &meta, prev, level, dir, &mut written)?)
        };
        if !patches.is_empty() {
            (feed.frames, feed.patches) = (frames, patches);
            feed.sign_patches(&name, &key);
        }
    }
    let tmp = dir.join(format!(".{}.tmp", update::FEED));
    fs::write(&tmp, serde_json::to_vec_pretty(&feed)?)?;
    fs::rename(&tmp, dir.join(update::FEED))?;
    // The install line's script, and a button for the developer's page.
    let script = Script::of(&meta, &u.feed, &feed.file, &sha256_file(bundle)?, size);
    for (name, text) in [(SCRIPT, script.text()), (BUTTON, button(&u.feed, &feed.file, &script.shown))] {
        let tmp = dir.join(format!(".{name}.tmp"));
        fs::write(&tmp, text)?;
        fs::rename(&tmp, dir.join(name))?;
        written.push(dir.join(name));
    }
    Ok(written)
}

/// The script the install line runs, beside the bundle in every release.
pub const SCRIPT: &str = "nami-install.sh";
/// A "Download for Linux" button to paste in the developer's page.
pub const BUTTON: &str = "nami-download-button.html";

/// The line people paste in a terminal: `curl -fsSL <feed>/install.sh | sh`.
pub fn install_line(feed: &str) -> String {
    format!("curl -fsSL {}/{SCRIPT} | sh", feed.trim_end_matches('/'))
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut File::open(path)?, &mut h)?;
    Ok(format::hex(&h.finalize()))
}

/// What the install script knows of the bundle it installs.
pub struct Script {
    pub name: String,
    /// As people see it (its menu entry's Name), and its command.
    pub shown: String,
    pub command: String,
    pub version: String,
    pub feed: String,
    pub file: String,
    pub sha256: String,
    pub size: u64,
    pub app: bool,
    pub developer: Option<String>,
    pub fingerprint: Option<String>,
}

/// `'text'`, for sh.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Script {
    pub fn of(meta: &format::Meta, feed: &str, file: &str, sha256: &str, size: u64) -> Script {
        let shown = meta.desktop.as_ref().and_then(|d| crate::bundle_menu::entry_key(&d.file, "Name")).filter(|n| !n.is_empty()).unwrap_or_else(|| meta.name.clone());
        Script {
            name: meta.name.clone(),
            shown,
            command: meta.entry.rsplit('/').next().filter(|c| !c.is_empty()).unwrap_or(&meta.name).to_string(),
            version: meta.version.clone(),
            feed: feed.trim_end_matches('/').to_string(),
            file: file.to_string(),
            sha256: sha256.to_string(),
            size,
            app: meta.desktop.is_some(),
            developer: meta.developer.clone(),
            fingerprint: meta.update.as_ref().and_then(|u| update::fingerprint(&u.key)),
        }
    }

    /// The script: downloads the bundle over HTTPS, checks it against its
    /// SHA-256 (this release's, written here), makes it a program in
    /// ~/.local/bin and adds it to the app menu with nami where it's here
    /// (namiOS, or nami on another distribution: it's kept in user.nami),
    /// else with the bundle's own --nami-bundle-install. Where it's
    /// installed already, the copy there updates itself instead: checked
    /// against the key trusted for it, so a release signed by someone else
    /// is refused.
    pub fn text(&self) -> String {
        let by = match (&self.developer, &self.fingerprint) {
            (Some(d), Some(f)) => format!("{} by {d}, its updates signed with the key {}", self.shown, crate::bundle_trust::short(f)),
            (Some(d), None) => format!("{} by {d}", self.shown),
            (None, Some(f)) => format!("{}, its updates signed with the key {}", self.shown, crate::bundle_trust::short(f)),
            (None, None) => self.shown.clone(),
        };
        let vars = [
            ("name", self.name.clone()),
            ("shown", self.shown.clone()),
            ("command", self.command.clone()),
            ("version", self.version.clone()),
            ("url", self.feed.clone()),
            ("file", self.file.clone()),
            ("sha256", self.sha256.clone()),
            ("size", self.size.to_string()),
            ("app", if self.app { "1".into() } else { "0".into() }),
            ("by", by),
        ];
        let mut out = String::from(INSTALL_HEAD);
        for (k, v) in vars {
            out += &format!("{k}={}\n", sh_quote(&v));
        }
        out += INSTALL_BODY;
        out
    }
}

const INSTALL_HEAD: &str = "#!/bin/sh
# Installs this app for you alone, with nothing else to install first:
#   curl -fsSL <this folder>/nami-install.sh | sh
# It downloads the app (one file, made with `nami bundle`), checks it
# against the SHA-256 below, puts it in ~/.local/bin and adds it to your
# apps. Run it again to update: the copy you have takes only an update
# signed with the key you trusted. `| sh -s -- --trust` trusts this
# release's key in place of that one, if its developer really changed it.
";

const INSTALL_BODY: &str = r#"
say() { printf '%s\n' "$*" >&2; }
fail() { say "$*"; exit 1; }
# The copy's own option: --tray-bundle-<what> for one made before nami
# (it knows no other, and starts its program with any other), else nami's.
own() {
  if head -c 16777216 "$1" 2>/dev/null | grep -qF -e --nami-bundle- || ! head -c 16777216 "$1" 2>/dev/null | grep -qF -e --tray-bundle-; then
    echo "--nami-bundle-$2"
  else
    echo "--tray-bundle-$2"
  fi
}

main() {
  trust=
  [ "${1:-}" = --trust ] && trust=1
  [ "$(uname -s)" = Linux ] || fail "$shown runs on Linux."
  case "$(uname -m)" in
    x86_64|amd64) ;;
    *) fail "$shown is made for x86_64 computers, and this one is $(uname -m)." ;;
  esac
  [ -n "${HOME:-}" ] || fail "HOME isn't set, so there's nowhere to install $shown."
  dir=$HOME/.local/bin
  dest=$dir/$command
  mkdir -p "$dir" || fail "Couldn't make $dir."

  # Installed already: it updates itself, checked against the key you trusted.
  if [ -f "$dest" ] && [ -z "$trust" ]; then
    if "$dest" "$(own "$dest" info)" </dev/null 2>/dev/null | head -1 | grep -q "^$name "; then
      say "$shown is installed already. Looking for a newer version…"
      "$dest" "$(own "$dest" update)" </dev/null || exit 1
      exit 0
    fi
    fail "$dest is there already and isn't $shown. Move it somewhere else, then try again."
  elif [ -f "$dest" ] && ! "$dest" "$(own "$dest" info)" </dev/null 2>/dev/null | head -1 | grep -q "^$name "; then
    fail "$dest is there already and isn't $shown. Move it somewhere else, then try again."
  fi

  if command -v curl >/dev/null 2>&1; then
    case "$url" in
      https://*) get() { curl -fL# --proto '=https' --tlsv1.2 --retry 2 -o "$2" "$1"; } ;;
      *) get() { curl -fL# --retry 2 -o "$2" "$1"; } ;;
    esac
  elif command -v wget >/dev/null 2>&1; then
    get() { wget -q -O "$2" "$1"; }
  else
    fail "Installing $shown needs curl or wget."
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    hash() { sha256sum "$1" | cut -d' ' -f1; }
  elif command -v shasum >/dev/null 2>&1; then
    hash() { shasum -a 256 "$1" | cut -d' ' -f1; }
  else
    hash() { openssl dgst -sha256 -r "$1" | cut -d' ' -f1; }
  fi

  tmp=$dir/.$command.download.$$
  trap 'rm -f "$tmp"' EXIT INT TERM
  say "Downloading $shown $version ($((size / 1048576 + 1)) MB)…"
  get "$url/$file" "$tmp" </dev/null || fail "Couldn't download $url/$file. Check your connection, then try again."
  [ "$(wc -c <"$tmp" | tr -d ' ')" = "$size" ] && [ "$(hash "$tmp")" = "$sha256" ] \
    || fail "The download doesn't match what the developer published, so nothing was installed. Try again later."
  chmod 755 "$tmp"
  "$tmp" --nami-bundle-verify </dev/null >/dev/null || fail "The download is damaged, so nothing was installed. Try again later."
  say "$by."
  [ -z "$trust" ] || "$tmp" --nami-bundle-trust </dev/null >/dev/null || fail "Couldn't trust its key."

  # With nami (namiOS, or nami on this distribution), it's kept in your user.nami too.
  if command -v nami >/dev/null 2>&1 && nami install "$tmp" </dev/null; then
    say "$shown is installed."
    exit 0
  fi
  mv "$tmp" "$dest"
  if ! NAMI_BUNDLE_ASK=0 "$dest" --nami-bundle-install </dev/null >/dev/null 2>"$tmp.said"; then
    rm -f "$dest"
    sed 's/ run the new copy once with --nami-bundle-trust, then install it again/ run this line again with "| sh -s -- --trust"/' "$tmp.said" >&2
    rm -f "$tmp.said"
    exit 1
  fi
  rm -f "$tmp.said"
  if [ "$app" = 1 ]; then
    say "$shown is installed. Open it from your apps."
  else
    say "$shown is installed. Run it with: $command"
    case ":$PATH:" in
      *":$dir:"*) ;;
      *) say "($dir isn't on your PATH yet: open a new terminal, or add it to your PATH.)" ;;
    esac
  fi
}

main "$@"
"#;

/// A "Download for Linux" button, and the install line, to paste in the
/// developer's page (its own styles: it looks the same anywhere).
pub fn button(feed: &str, file: &str, shown: &str) -> String {
    let feed = feed.trim_end_matches('/');
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    format!(
        r#"<!-- {shown} for Linux: a download button and the install line (made by nami bundle). -->
<div style="font-family: system-ui, -apple-system, sans-serif; max-width: 36rem">
  <a href="{feed}/{file}" download style="display: inline-block; padding: .65em 1.3em; border-radius: 999px; background: #18181b; color: #fff; font-weight: 600; text-decoration: none">Download for Linux</a>
  <span style="margin-left: .7em; color: #6f6f78; font-size: .9em">x86_64 · Ubuntu, Fedora, Debian, Arch and more</span>
  <p style="margin: 1em 0 .4em; color: #52525b; font-size: .9em">Or install it from a terminal:</p>
  <pre style="margin: 0; padding: .7em .9em; border-radius: 10px; background: #f4f4f5; color: #18181b; overflow-x: auto"><code>{line}</code></pre>
</div>
"#,
        shown = esc(shown),
        feed = esc(feed),
        file = esc(file),
        line = esc(&install_line(feed)),
    )
}

/// The release before this one, whose parts patches are made from.
struct Previous {
    feed: Feed,
    meta: format::Meta,
    /// Its description's sha256.
    hash: String,
    from: From,
}

enum From {
    /// The bundle file itself.
    File(PathBuf),
    /// The folder it was published from.
    Dir(PathBuf),
    /// Where it's served (the --update-url folder).
    Url(String, ureq::Agent),
}

impl Previous {
    /// `--previous`: the earlier release's bundle file, the folder it was
    /// published from, or its URL; `none` for no patches. Else what `dir`
    /// has from the last time, or what the update URL (`feed`) serves now:
    /// a first release has none.
    fn find(arg: Option<&str>, dir: &Path, feed: &str) -> Result<Option<Previous>> {
        let found = match arg.map(str::trim) {
            Some("none") => return Ok(None),
            Some(a) if a.starts_with("https://") || a.starts_with("http://") || a.starts_with("github:") => Previous::url(&update::feed_url(a)).with_context(|| format!("--previous {a}"))?,
            Some(a) if Path::new(a).is_dir() => Previous::dir(Path::new(a)).with_context(|| format!("--previous {a}"))?,
            Some(a) => Previous::file(Path::new(a)).with_context(|| format!("--previous {a}"))?,
            None if dir.join(update::FEED).exists() => match Previous::dir(dir) {
                Ok(p) => p,
                Err(e) => {
                    crate::say!("no patches: {} has a release that can't be read ({e:#})", dir.display());
                    return Ok(None);
                }
            },
            None => match Previous::url(feed) {
                Ok(p) => p,
                Err(e) => {
                    crate::say!("no patches: no earlier release at {feed} ({e:#}); --previous says where it is");
                    return Ok(None);
                }
            },
        };
        Ok(Some(found))
    }

    fn file(path: &Path) -> Result<Previous> {
        let (trailer, meta) = format::read(&mut File::open(path).with_context(|| format!("opening {}", path.display()))?)?;
        let feed = Feed { name: meta.name.clone(), version: meta.version.clone(), sha256: trailer.hash(), ..Default::default() };
        Ok(Previous { feed, meta, hash: trailer.hash(), from: From::File(path.to_path_buf()) })
    }

    fn dir(dir: &Path) -> Result<Previous> {
        let feed: Feed = serde_json::from_slice(&fs::read(dir.join(update::FEED))?).context("its update.json")?;
        let meta = fs::read(dir.join(update::meta_file(&feed.sha256))).context("its description")?;
        Previous::with(feed, &meta, From::Dir(dir.to_path_buf()))
    }

    fn url(url: &str) -> Result<Previous> {
        let agent = ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(15)).timeout_read(std::time::Duration::from_secs(60)).build();
        let get = |file: &str, limit: u64| -> Result<Vec<u8>> {
            let mut data = Vec::new();
            agent.get(&format!("{url}/{file}")).call()?.into_reader().take(limit).read_to_end(&mut data)?;
            Ok(data)
        };
        let feed: Feed = serde_json::from_slice(&get(update::FEED, 1 << 20)?).context("its update.json")?;
        let meta = get(&update::meta_file(&feed.sha256), 64 << 20)?;
        Previous::with(feed, &meta, From::Url(url.to_string(), agent))
    }

    fn with(feed: Feed, meta: &[u8], from: From) -> Result<Previous> {
        ensure!(format::sha256_hex(meta) == feed.sha256, "its description isn't the one its update.json names");
        let meta: format::Meta = serde_json::from_slice(meta).context("its description")?;
        Ok(Previous { hash: feed.sha256.clone(), feed, meta, from })
    }

    /// Its frame of `p`, checked.
    fn frame(&self, p: &format::PathEntry) -> Result<Vec<u8>> {
        let data = match &self.from {
            From::File(path) => {
                let mut f = File::open(path)?;
                f.seek(SeekFrom::Start(p.offset))?;
                let mut d = Vec::new();
                f.take(p.size).read_to_end(&mut d)?;
                d
            }
            From::Dir(dir) => fs::read(dir.join(update::part_file(&p.sha256)))?,
            From::Url(url, agent) => {
                let mut d = Vec::new();
                agent.get(&format!("{url}/{}", update::part_file(&p.sha256))).call()?.into_reader().take(p.size + 1).read_to_end(&mut d)?;
                d
            }
        };
        ensure!(data.len() as u64 == p.size && format::sha256_hex(&data) == p.sha256, "its part {} isn't what its description says", p.name);
        Ok(data)
    }
}

/// For each part that changed since `prev`, a patch from that release's
/// part of the same kind (bundle_delta::kind), where it's smaller than the
/// part: written to `dir`, each checked by making the part from it as a
/// runtime would.
fn make_patches(bundle: &Path, meta: &format::Meta, prev: &Previous, level: i32, dir: &Path, written: &mut Vec<PathBuf>) -> Result<Vec<delta::Patch>> {
    use std::collections::{HashMap, HashSet};
    let had: HashSet<&str> = prev.meta.paths.iter().filter(|p| !p.fetch).map(|p| p.sha256.as_str()).collect();
    let mut by_kind: HashMap<String, Vec<&format::PathEntry>> = HashMap::new();
    for p in prev.meta.paths.iter().filter(|p| !p.fetch) {
        by_kind.entry(delta::kind(&p.name)).or_default().push(p);
    }
    // Each changed part, and the old one nearest its size.
    let jobs: Vec<(&format::PathEntry, &format::PathEntry)> = meta
        .paths
        .iter()
        .filter(|p| !p.fetch && !had.contains(p.sha256.as_str()))
        .filter_map(|p| {
            let old = by_kind.get(&delta::kind(&p.name))?.iter().min_by_key(|o| o.nar_size.abs_diff(p.nar_size))?;
            (delta::memory(old.nar_size, p.nar_size) <= delta::MAX_MEMORY).then_some((p, *old))
        })
        .collect();
    let changed = meta.paths.iter().filter(|p| !p.fetch && !had.contains(p.sha256.as_str())).count();
    let what = if prev.meta.version.is_empty() { prev.hash[..12].to_string() } else { prev.meta.version.clone() };
    if changed == 0 {
        return Ok(Vec::new());
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<Option<delta::Patch>>>>> = jobs.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4);
    std::thread::scope(|s| {
        for _ in 0..threads.min(jobs.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((p, old)) = jobs.get(i) else { break };
                let r = (|| -> Result<Option<delta::Patch>> {
                    let old_nar = delta::decompress(&prev.frame(old)?, old.nar_size)?;
                    let mut f = File::open(bundle)?;
                    f.seek(SeekFrom::Start(p.offset))?;
                    let mut frame = Vec::new();
                    (&mut f).take(p.size).read_to_end(&mut frame)?;
                    let new_nar = delta::decompress(&frame, p.nar_size)?;
                    let data = delta::make(&old_nar, &new_nar, level)?;
                    drop(new_nar);
                    // Worth it, and it comes out as the part.
                    if data.len() as u64 * 10 > p.size * 9 {
                        return Ok(None);
                    }
                    let (sha, size) = delta::apply(&old_nar, &data[..], p.nar_size, &delta::Frames::here(level), std::io::sink())?;
                    if sha != p.sha256 || size != p.size {
                        return Ok(None);
                    }
                    let patch = delta::Patch { from: old.sha256.clone(), to: p.sha256.clone(), size: data.len() as u64, sha256: format::sha256_hex(&data) };
                    let dest = dir.join(patch.file());
                    if !dest.metadata().is_ok_and(|m| m.len() == patch.size) {
                        let tmp = dir.join(format!(".{}.tmp", patch.file()));
                        fs::write(&tmp, &data)?;
                        fs::rename(&tmp, &dest)?;
                    }
                    Ok(Some(patch))
                })();
                *results[i].lock().unwrap() = Some(r);
            });
        }
    });
    let mut patches = Vec::new();
    for ((p, _), r) in jobs.iter().zip(results) {
        match r.into_inner().unwrap().unwrap() {
            Ok(Some(patch)) => {
                written.push(dir.join(patch.file()));
                patches.push(patch);
            }
            Ok(None) => {}
            Err(e) => crate::say!("no patch for {}: {e:#}", p.name),
        }
    }
    let whole: u64 = patches.iter().map(|x| meta.paths.iter().find(|p| p.sha256 == x.to).map_or(0, |p| p.size)).sum();
    let small: u64 = patches.iter().map(|x| x.size).sum();
    let mib = |b: u64| b as f64 / 1048576.0;
    crate::say!(
        "since {what}: {changed} of its {} parts changed; {} of them as patches, {:.1} MiB in place of {:.1} MiB",
        meta.paths.iter().filter(|p| !p.fetch).count(),
        patches.len(),
        mib(small),
        mib(whole)
    );
    Ok(patches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle_format::{Meta, PathEntry, Trailer};
    use crate::bundle_update::Me;

    fn data(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|i| {
                x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                if i % 5 < 2 {
                    (x >> 33) as u8
                } else {
                    b"a program's bytes "[i % 18]
                }
            })
            .collect()
    }

    /// A bundle of `parts` (name, NAR), version `version`, that updates from `feed`.
    fn bundle(out: &Path, parts: &[(&str, &[u8])], version: &str, serial: u64, feed: &str, key: &SigningKey) -> (Vec<u8>, Trailer) {
        let runtime = b"#!/bin/sh\nexit 0\n".to_vec();
        let mut file = runtime.clone();
        let mut paths = Vec::new();
        for (name, nar) in parts {
            let mut enc = delta::frame_encoder(Vec::new(), 3, nar.len() as u64).unwrap();
            enc.write_all(nar).unwrap();
            let frame = enc.finish().unwrap();
            paths.push(PathEntry { name: format!("{}-{name}", "b".repeat(32)), offset: file.len() as u64, size: frame.len() as u64, sha256: format::sha256_hex(&frame), nar_size: nar.len() as u64, ..Default::default() });
            file.extend(frame);
        }
        let meta = Meta { format: 1, name: "deltademo".into(), version: version.into(), entry: "/nix/store/x/bin/d".into(), paths, runtime: format::sha256_hex(&runtime), update: Some(Update { feed: feed.into(), key: update::public_key("k", key), serial, rotations: vec![] }), ..Default::default() };
        let json = serde_json::to_vec(&meta).unwrap();
        let trailer = Trailer { payload_offset: runtime.len() as u64, meta_offset: file.len() as u64, meta_len: json.len() as u64, meta_sha256: sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(&json)).into() };
        file.extend(&json);
        file.extend(trailer.encode());
        fs::write(out, &file).unwrap();
        (json, trailer)
    }

    /// Serves `dir` on 127.0.0.1, counting the bytes it sends.
    fn serve(dir: PathBuf, sent: std::sync::Arc<std::sync::atomic::AtomicU64>) -> String {
        use std::io::BufRead;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let (dir, sent) = (dir.clone(), sent.clone());
                std::thread::spawn(move || {
                    let mut r = std::io::BufReader::new(&s);
                    let mut first = String::new();
                    let _ = r.read_line(&mut first);
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                    }
                    let path = first.split(' ').nth(1).unwrap_or("/").trim_start_matches('/').to_string();
                    let mut w = &s;
                    match fs::read(dir.join(&path)).ok().filter(|_| !path.contains("..")) {
                        Some(b) => {
                            sent.fetch_add(b.len() as u64, std::sync::atomic::Ordering::Relaxed);
                            let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                            let _ = w.write_all(&b);
                        }
                        None => {
                            let _ = write!(w, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        url
    }

    #[test]
    fn an_update_downloads_only_what_changed_and_comes_out_whole() {
        let tmp = std::env::temp_dir().join(format!("nami-delta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let (v1dir, v2dir, home) = (tmp.join("v1"), tmp.join("v2"), tmp.join("home"));
        for d in [&v1dir, &v2dir, &home] {
            fs::create_dir_all(d).unwrap();
        }
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let url = serve(v2dir.clone(), sent.clone());
        // Version 2 changes the app a little; its library stays.
        let lib = data(300_000, 1);
        let app1 = data(1 << 20, 2);
        let mut app2 = app1.clone();
        app2.splice(500_000..500_000, data(30_000, 3));
        app2[100..200].copy_from_slice(&data(100, 4));
        let (_, t1) = bundle(&v1dir.join("deltademo-x86_64"), &[("libfoo-1.0", &lib), ("deltademo-1.0", &app1)], "1.0", 1000, &url, &key);
        let (json2, t2) = bundle(&v2dir.join("deltademo-x86_64"), &[("libfoo-1.0", &lib), ("deltademo-2.0", &app2)], "2.0", 2000, &url, &key);
        let v2 = v2dir.join("deltademo-x86_64");
        let (_, m2) = format::read(&mut File::open(&v2).unwrap()).unwrap();

        // The patches, from version 1's file.
        let prev = Previous::find(Some(v1dir.join("deltademo-x86_64").to_str().unwrap()), &v2dir, &url).unwrap().unwrap();
        assert_eq!(prev.hash, t1.hash());
        let mut written = Vec::new();
        let patches = make_patches(&v2, &m2, &prev, 3, &v2dir, &mut written).unwrap();
        assert_eq!(patches.len(), 1, "the app's part only: the library didn't change");
        let app_part = &m2.paths[1];
        assert_eq!(patches[0].to, app_part.sha256);
        assert!(patches[0].size * 3 < app_part.size, "a patch of {} bytes for a part of {}", patches[0].size, app_part.size);
        assert!(v2dir.join(patches[0].file()).exists());
        // Version 2 published: its parts, description and signed feed.
        for p in &m2.paths {
            let mut f = File::open(&v2).unwrap();
            f.seek(SeekFrom::Start(p.offset)).unwrap();
            let mut d = Vec::new();
            f.take(p.size).read_to_end(&mut d).unwrap();
            fs::write(v2dir.join(update::part_file(&p.sha256)), d).unwrap();
        }
        fs::write(v2dir.join(update::meta_file(&t2.hash())), &json2).unwrap();
        let mut feed = Feed { format: 1, name: "deltademo".into(), version: "2.0".into(), serial: 2000, sha256: t2.hash(), size: fs::metadata(&v2).unwrap().len(), file: "deltademo-x86_64".into(), ..Default::default() };
        feed.sign("k", &key);
        feed.frames = Some(delta::Frames::here(3));
        feed.patches = patches.clone();
        feed.sign_patches("k", &key);
        let write_feed = |f: &Feed| fs::write(v2dir.join(update::FEED), serde_json::to_vec(f).unwrap()).unwrap();
        write_feed(&feed);

        // A copy of version 1 updates: the patch, not the part.
        let update_as = |sent: &std::sync::atomic::AtomicU64, choice: delta::Choice| {
            let copy = home.join("deltademo-x86_64");
            fs::copy(v1dir.join("deltademo-x86_64"), &copy).unwrap();
            let _ = fs::remove_file(update::staged(&copy));
            let (t, meta) = format::read(&mut File::open(&copy).unwrap()).unwrap();
            let hash = t.hash();
            sent.store(0, std::sync::atomic::Ordering::Relaxed);
            let me = Me { path: &copy, meta: &meta, hash: &hash, cache: &home.join("cache") };
            let outcome = me.check(choice).unwrap();
            assert!(fs::read(update::staged(&copy)).unwrap() == fs::read(&v2).unwrap(), "byte for byte the developer's");
            (outcome, sent.load(std::sync::atomic::Ordering::Relaxed))
        };
        let update_once = |sent: &std::sync::atomic::AtomicU64| update_as(sent, delta::Choice::Always);
        // Never: the whole part. Someone waits (Auto): whichever the link it measures makes
        // quicker; on a busy machine even this local one can measure slow, so either way, as
        // long as it comes out whole (update_as checks it's byte for byte the developer's).
        match update_as(&sent, delta::Choice::Never).0 {
            update::Outcome::Staged { downloaded: 1, patched: 0, .. } => {}
            o => panic!("Never: {o:?}"),
        }
        match update_as(&sent, delta::Choice::Auto).0 {
            update::Outcome::Staged { downloaded: 1, .. } => {}
            o => panic!("Auto: {o:?}"),
        }
        let (outcome, bytes) = update_once(&sent);
        match outcome {
            update::Outcome::Staged { downloaded: 1, patched: 1, bytes, .. } => assert_eq!(bytes, patches[0].size),
            o => panic!("{o:?}"),
        }
        // (What the server sent: the feed, the description and the patch.)
        assert!(bytes < patches[0].size + json2.len() as u64 + 4096, "{bytes}");

        // Patches signed by no one, or changed: the whole part, as before.
        let mut forged = feed.clone();
        forged.patches[0].size += 1;
        write_feed(&forged);
        match update_once(&sent).0 {
            update::Outcome::Staged { downloaded: 1, patched: 0, bytes, .. } => assert_eq!(bytes, app_part.size),
            o => panic!("{o:?}"),
        }
        // A feed with no patches (an older nami's): the same.
        let mut old = feed.clone();
        (old.frames, old.patches, old.patches_signature) = (None, vec![], String::new());
        write_feed(&old);
        assert!(matches!(update_once(&sent).0, update::Outcome::Staged { patched: 0, .. }));
        // A patch file that's been changed: refused by its hash, and the part comes whole.
        write_feed(&feed);
        fs::write(v2dir.join(patches[0].file()), vec![0u8; patches[0].size as usize]).unwrap();
        assert!(matches!(update_once(&sent).0, update::Outcome::Staged { downloaded: 1, patched: 0, .. }));

        // An older runtime reads the new feed and checks it as ever: the
        // signature over name, serial and hash is the same.
        let text = serde_json::to_string(&feed).unwrap();
        assert!(text.contains("\"patches\""));
        let back: Feed = serde_json::from_str(&text).unwrap();
        back.verify(&update::public_key("k", &key)).unwrap();
        fs::remove_dir_all(&tmp).unwrap();
    }
}

