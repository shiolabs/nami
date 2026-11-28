//! Secrets, written right in nami files:
//!
//! ```yaml
//! env:
//!   STRIPE_KEY: !secret "YWdlLWVuY3J5cHRpb24ub3JnL3Yx..."
//! ```
//!
//! A value tagged `!secret` is encrypted with age, to everyone listed in the
//! repo's `.nami-recipients` (people's keys: their SSH key or a key nami
//! made; and computers' keys, made by `nami usb`). Written plain
//! (`!secret hunter2`), `nami secret seal` encrypts it in place, changing
//! nothing else in the file. The ciphertext is one line of base64, safe to
//! commit.
//!
//! When a nami file is read, each sealed secret becomes a marked string
//! (`!secret <ciphertext>`), so the rest of nami carries it along without
//! knowing; only the places that take secrets decrypt it, into memory or
//! files of their own (never the store, which everyone can read).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use age::secrecy::ExposeSecret;
use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine;
use serde_yaml::Value;

/// How a sealed secret reads once its file is loaded.
pub const MARK: &str = "!secret ";
/// The file listing who can read a repo's secrets.
pub const RECIPIENTS: &str = ".nami-recipients";
/// An age file starts with this; base64'd, so does a sealed secret.
const AGE_HEADER: &[u8] = b"age-encryption.org/v1\n";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Is `s` a sealed secret (base64 of an age file)?
pub fn is_sealed(s: &str) -> bool {
    s.starts_with("YWdlLWVuY3J5cHRpb24ub3JnL3Yx") && B64.decode(s.trim()).is_ok_and(|d| d.starts_with(AGE_HEADER))
}

/// A sealed secret as nami carries it, or None for any other string.
pub fn marked(s: &str) -> Option<&str> {
    s.strip_prefix(MARK).filter(|c| is_sealed(c))
}

/// Does `v` hold a secret anywhere?
pub fn contains(v: &Value) -> bool {
    match v {
        Value::String(s) => marked(s).is_some(),
        Value::Mapping(m) => m.values().any(contains),
        Value::Sequence(s) => s.iter().any(contains),
        Value::Tagged(t) => contains(&t.value),
        _ => false,
    }
}

// --- Keys -------------------------------------------------------------------

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

/// Your own key, if nami made you one: ~/.config/nami/age.key.
pub fn nami_key_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
        .join("nami/age.key")
}

/// Your SSH keys age can use (ed25519 and RSA).
fn ssh_key_paths() -> Vec<PathBuf> {
    ["id_ed25519", "id_rsa"].iter().map(|k| home().join(".ssh").join(k)).collect()
}

/// A computer's own key, made by `nami usb` (root's alone), in nami's folder: /nami/secrets/age.key.
pub const COMPUTER_KEY: &str = "secrets/age.key";

/// The computer at `root`'s own key.
pub fn computer_key_at(root: &Path) -> PathBuf {
    crate::migrate::var_dir(root).join(COMPUTER_KEY)
}

pub fn computer_key_path() -> PathBuf {
    computer_key_at(&PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into())))
}

/// Where the computers you make keep their keys here, before they're on them (each
/// `<host>.age.key`), and what else making them keeps: ~/.cache/nami/os, or NAMI_OS_DIR.
pub fn computer_key_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NAMI_OS_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return dir;
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache"))
        .join("nami/os")
}

/// An age key file's secret key.
fn read_age_key(path: &Path) -> Result<age::x25519::Identity> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let line = text.lines().find(|l| l.starts_with("AGE-SECRET-KEY-")).ok_or_else(|| anyhow!("{} holds no age key", path.display()))?;
    age::x25519::Identity::from_str(line.trim()).map_err(|e| anyhow!("{}: {e}", path.display()))
}

/// Make an age key at `path` (private to its owner); returns its public key.
pub fn make_age_key(path: &Path, what: &str) -> Result<String> {
    if let Ok(id) = read_age_key(path) {
        return Ok(id.to_public().to_string());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let id = age::x25519::Identity::generate();
    let public = id.to_public().to_string();
    let tmp = path.with_extension("part");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    writeln!(f, "# {what}. Keep it private.\n# public key: {public}\n{}", id.to_string().expose_secret())?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(public)
}

/// The public key of an age key file.
pub fn public_of(path: &Path) -> Result<String> {
    Ok(read_age_key(path)?.to_public().to_string())
}

/// Passphrases for SSH keys, asked on the terminal.
#[derive(Clone)]
struct Ask;

impl age::Callbacks for Ask {
    fn display_message(&self, message: &str) {
        eprintln!("{message}");
    }
    fn confirm(&self, _: &str, _: &str, _: Option<&str>) -> Option<bool> {
        None
    }
    fn request_public_string(&self, _: &str) -> Option<String> {
        None
    }
    fn request_passphrase(&self, description: &str) -> Option<age::secrecy::SecretString> {
        if !crate::errors::may_ask() {
            return None;
        }
        crate::errors::read_hidden(&format!("{description}: ")).ok().map(|s| s.into())
    }
}

/// The keys this person (or computer) can open secrets with.
pub struct Keys {
    ids: Vec<Box<dyn age::Identity>>,
    /// Where they are, for messages.
    pub names: Vec<String>,
}

impl Keys {
    /// Yours: nami's key, your SSH keys, and the computer's if you can read it (root).
    pub fn mine() -> Keys {
        let mut keys = Keys { ids: Vec::new(), names: Vec::new() };
        for p in [nami_key_path(), computer_key_path()] {
            if let Ok(id) = read_age_key(&p) {
                keys.ids.push(Box::new(id));
                keys.names.push(p.display().to_string());
            }
        }
        for p in ssh_key_paths() {
            let Ok(f) = fs::File::open(&p) else { continue };
            match age::ssh::Identity::from_buffer(std::io::BufReader::new(f), Some(p.display().to_string())) {
                Ok(age::ssh::Identity::Unsupported(_)) | Err(_) => {}
                Ok(id) => {
                    keys.ids.push(Box::new(id.with_callbacks(Ask)));
                    keys.names.push(p.display().to_string());
                }
            }
        }
        keys
    }

    /// Yours that open secrets without asking anything (nami's key, the
    /// computer's if you can read it, SSH keys without a passphrase), and
    /// the ones that would ask, or can't be read, and why.
    pub fn without_asking() -> (Keys, Vec<String>) {
        let mut keys = Keys { ids: Vec::new(), names: Vec::new() };
        let mut untried = Vec::new();
        for p in [nami_key_path(), computer_key_path()] {
            match read_age_key(&p) {
                Ok(id) => {
                    keys.ids.push(Box::new(id));
                    keys.names.push(p.display().to_string());
                }
                // The computer's is root's alone (its folder too).
                Err(_) if p.parent().is_some_and(|d| d.is_dir() && fs::read_dir(d).is_err()) || (p.exists() && fs::File::open(&p).is_err()) => {
                    untried.push(format!("{} (root's: `sudo nami check` tries it)", p.display()));
                }
                Err(_) => {}
            }
        }
        for p in ssh_key_paths() {
            let Ok(f) = fs::File::open(&p) else { continue };
            match age::ssh::Identity::from_buffer(std::io::BufReader::new(f), Some(p.display().to_string())) {
                Ok(id @ age::ssh::Identity::Unencrypted(_)) => {
                    keys.ids.push(Box::new(id));
                    keys.names.push(p.display().to_string());
                }
                Ok(age::ssh::Identity::Encrypted(_)) => untried.push(format!("{} (it has a passphrase: `nami secret show` asks for it)", p.display())),
                _ => {}
            }
        }
        (keys, untried)
    }

    /// Whether these keys open `sealed`.
    pub fn opens(&self, sealed: &str) -> bool {
        !self.ids.is_empty() && self.decrypt(sealed).is_ok()
    }

    /// Just this key (tests).
    #[cfg(test)]
    pub fn of(id: age::x25519::Identity) -> Keys {
        Keys { ids: vec![Box::new(id)], names: vec!["test".into()] }
    }

    /// Only the key in `path` (a computer's).
    pub fn file(path: &Path) -> Result<Keys> {
        Ok(Keys { ids: vec![Box::new(read_age_key(path)?)], names: vec![path.display().to_string()] })
    }

    /// Open a sealed secret.
    pub fn decrypt(&self, sealed: &str) -> Result<Vec<u8>> {
        ensure!(!self.ids.is_empty(), "you have no key to open secrets with (`nami secret key` makes one)");
        let data = B64.decode(sealed.trim()).map_err(|_| anyhow!("it isn't a sealed secret"))?;
        let d = age::Decryptor::new(&data[..]).map_err(|e| anyhow!("it isn't a sealed secret: {e}"))?;
        let mut r = d.decrypt(self.ids.iter().map(|i| i.as_ref() as &dyn age::Identity)).map_err(|e| match e {
            age::DecryptError::NoMatchingKeys => anyhow!("none of your keys can open it (tried {})", self.names.join(", ")),
            e => anyhow!("{e}"),
        })?;
        let mut out = Vec::new();
        r.read_to_end(&mut out)?;
        Ok(out)
    }

    /// Open a sealed secret that's text.
    pub fn decrypt_text(&self, sealed: &str) -> Result<String> {
        String::from_utf8(self.decrypt(sealed)?).map_err(|_| anyhow!("it isn't text"))
    }

    /// `s` itself, or what it says if it's a secret.
    pub fn reveal(&self, s: &str) -> Result<String> {
        match marked(s) {
            Some(c) => self.decrypt_text(c),
            None => Ok(s.to_string()),
        }
    }

    /// `v` with every secret in it opened.
    pub fn reveal_value(&self, v: &Value) -> Result<Value> {
        Ok(match v {
            Value::String(s) => Value::String(self.reveal(s)?),
            Value::Mapping(m) => Value::Mapping(m.iter().map(|(k, v)| Ok((k.clone(), self.reveal_value(v)?))).collect::<Result<_>>()?),
            Value::Sequence(s) => Value::Sequence(s.iter().map(|v| self.reveal_value(v)).collect::<Result<_>>()?),
            other => other.clone(),
        })
    }
}

/// What to do when you can't open a secret.
pub fn access_hint() -> String {
    "someone who can open it runs `nami secret allow <your name> <your key>` in the repo and pushes (`nami secret key` prints your key)".into()
}

/// Your public key, for `nami secret allow` (made, if you have none and
/// `make`): nami's key, else your SSH key.
pub fn my_public_key(make: bool) -> Result<Option<String>> {
    if let Ok(p) = public_of(&nami_key_path()) {
        return Ok(Some(p));
    }
    for p in ssh_key_paths() {
        let public = p.with_extension("pub");
        if p.is_file() {
            if let Ok(text) = fs::read_to_string(&public) {
                let key: Vec<&str> = text.split_whitespace().take(2).collect();
                if key.len() == 2 && recipient(&key.join(" ")).is_ok() {
                    return Ok(Some(key.join(" ")));
                }
            }
        }
    }
    if !make {
        return Ok(None);
    }
    let path = nami_key_path();
    let public = make_age_key(&path, "Your nami key, to open the secrets in nami files")?;
    if let Some(d) = path.parent() {
        crate::machine::ignore_own_files(d);
    }
    eprintln!("made your key: {} (back it up; it never goes into git)", path.display());
    Ok(Some(public))
}

// --- Recipients ---------------------------------------------------------------

/// A public key: `age1...`, or an SSH key (`ssh-ed25519 AAAA...`, `ssh-rsa AAAA...`).
pub fn recipient(key: &str) -> Result<Box<dyn age::Recipient + Send>> {
    let key = key.trim();
    if key.starts_with("age1") {
        return age::x25519::Recipient::from_str(key).map(|r| Box::new(r) as _).map_err(|e| anyhow!("{key:?} isn't an age key: {e}"));
    }
    if key.starts_with("ssh-") {
        let two: Vec<&str> = key.split_whitespace().take(2).collect();
        return age::ssh::Recipient::from_str(&two.join(" ")).map(|r| Box::new(r) as _).map_err(|e| anyhow!("{key:?} isn't an SSH key age can use: {e:?}"));
    }
    bail!("{key:?} isn't a public key: an age key (age1...) or an SSH key (ssh-ed25519 ..., ssh-rsa ...)")
}

/// The `.nami-recipients` that covers `path`: in its folder or one above,
/// up to the repo's top (the folder with .git).
pub fn recipients_file(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path.to_path_buf() } else { path.parent().unwrap_or(Path::new(".")).to_path_buf() };
    let mut dir = fs::canonicalize(&start).unwrap_or(start);
    loop {
        if crate::oldnames::is_file(&dir.join(RECIPIENTS)) {
            return Some(crate::oldnames::find_in(&dir, RECIPIENTS));
        }
        if dir.join(".git").exists() || !dir.pop() {
            return None;
        }
    }
}

/// Where a new `.nami-recipients` for `path` goes: the top of its repo, or its folder.
fn new_recipients_file(path: &Path) -> PathBuf {
    let start = if path.is_dir() { path.to_path_buf() } else { path.parent().unwrap_or(Path::new(".")).to_path_buf() };
    let start = fs::canonicalize(&start).unwrap_or(start);
    let mut dir = start.clone();
    loop {
        if dir.join(".git").exists() {
            return dir.join(RECIPIENTS);
        }
        if !dir.pop() {
            return start.join(RECIPIENTS);
        }
    }
}

/// `name key` lines; `#` starts a comment.
pub fn parse_recipients(text: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split(" #").next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, key) = line.split_once(char::is_whitespace).ok_or_else(|| anyhow!("line {}: a name, then a public key", n + 1))?;
        recipient(key).with_context(|| format!("line {}", n + 1))?;
        out.push((name.to_string(), key.trim().to_string()));
    }
    Ok(out)
}

/// The recipients for secrets in `path`, and their file.
pub fn recipients_of(path: &Path) -> Result<Option<(PathBuf, Vec<(String, String)>)>> {
    let Some(file) = recipients_file(path) else { return Ok(None) };
    let list = parse_recipients(&fs::read_to_string(&file)?).with_context(|| file.display().to_string())?;
    Ok(Some((file, list)))
}

/// The recipients for `path`, making `.nami-recipients` with your key if
/// there's none yet.
fn recipients_or_make(path: &Path) -> Result<(PathBuf, Vec<(String, String)>)> {
    if let Some(r) = recipients_of(path)? {
        ensure!(!r.1.is_empty(), "{} lists nobody: add yourself with `nami secret allow <name> <key>`", r.0.display());
        return Ok(r);
    }
    let me = my_public_key(true)?.expect("made");
    let name = std::env::var("USER").ok().filter(|u| !u.is_empty() && !u.contains(char::is_whitespace)).unwrap_or_else(|| "me".into());
    let file = new_recipients_file(path);
    fs::write(&file, recipients_text("", &[(name.clone(), me.clone())]))?;
    eprintln!("made {}: secrets here are for {name} (you) to open; `nami secret allow` adds others", file.display());
    Ok((file, vec![(name, me)]))
}

/// The recipients file's text with `list` in it: lines kept where they
/// are, changed in place, new ones at the end, removed ones gone.
fn recipients_text(old: &str, list: &[(String, String)]) -> String {
    let mut out = String::new();
    if old.is_empty() {
        out += "# Who can open the secrets in this repo's nami files: a name, then a public\n# key (age, or SSH). Change it with `nami secret allow`, which encrypts them\n# again for whoever is here.\n";
    }
    let mut done = Vec::new();
    for line in old.lines() {
        let body = line.split(" #").next().unwrap_or("").trim();
        let name = body.split_whitespace().next().filter(|_| !body.starts_with('#'));
        match name {
            Some(n) => {
                if let Some((n, k)) = list.iter().find(|(x, _)| x == n) {
                    if !done.contains(n) {
                        out += &format!("{n} {k}\n");
                        done.push(n.clone());
                    }
                }
            }
            None => out += &format!("{line}\n"),
        }
    }
    for (n, k) in list.iter().filter(|(n, _)| !done.contains(n)) {
        out += &format!("{n} {k}\n");
    }
    out
}

fn encrypt(plain: &[u8], recipients: &[(String, String)]) -> Result<String> {
    let keys: Vec<Box<dyn age::Recipient + Send>> = recipients.iter().map(|(_, k)| recipient(k)).collect::<Result<_>>()?;
    ensure!(!keys.is_empty(), "nobody to encrypt to");
    let e = age::Encryptor::with_recipients(keys.iter().map(|k| k.as_ref() as &dyn age::Recipient)).map_err(|e| anyhow!("{e}"))?;
    let mut out = Vec::new();
    let mut w = e.wrap_output(&mut out)?;
    w.write_all(plain)?;
    w.finish()?;
    Ok(B64.encode(out))
}

/// `plain` sealed for one public key, as a loaded file carries it (tests).
#[cfg(test)]
pub fn marked_for(plain: &str, key: &str) -> String {
    format!("{MARK}{}", encrypt(plain.as_bytes(), &[("k".into(), key.into())]).unwrap())
}

// --- Secrets in a file's text -------------------------------------------------

/// A `!secret` in a file's text.
#[derive(Debug, Clone)]
pub struct Found {
    /// Where `!secret` starts, and where its value ends.
    pub start: usize,
    pub end: usize,
    /// 1-based.
    pub line: usize,
    /// The column a block scalar's lines are deeper than.
    col: usize,
    /// Inside `{ }` or `[ ]`.
    flow: bool,
    /// Where it is: `env.STRIPE_KEY`.
    pub path: String,
    /// The ciphertext if sealed, else the plain value.
    pub value: String,
    pub sealed: bool,
}

/// Where each `!secret` in `text` starts and ends (its value's end), and the
/// column its block scalar would be under, and whether it's in a flow.
fn spans(text: &str) -> Result<Vec<(usize, usize, usize, bool)>> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let (mut i, mut flow, mut at_start) = (0usize, 0i32, true);
    let mut line_start = 0usize;
    // The column of the last key or `-` on this line: a block scalar's
    // lines go deeper than it.
    let mut parent = 0usize;
    let ws = |c: u8| c == b' ' || c == b'\t' || c == b'\n' || c == b'\r';
    let at = |i: usize| if i < n { b[i] } else { b'\n' };
    while i < n {
        let c = b[i];
        if c == b'\n' {
            i += 1;
            line_start = i;
            at_start = true;
            let mut j = i;
            while j < n && b[j] == b' ' {
                j += 1;
            }
            parent = j - line_start;
            // `---`, `...` and directives: the whole line.
            if b[i..].starts_with(b"---") || b[i..].starts_with(b"...") || at(i) == b'%' {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            continue;
        }
        if i == 0 && (b.starts_with(b"---") || b.starts_with(b"%")) {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        match c {
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' => {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'-' | b'?' if at_start && ws(at(i + 1)) => {
                parent = i - line_start;
                i += 1;
            }
            b'[' | b'{' => {
                flow += 1;
                i += 1;
                at_start = true;
            }
            b']' | b'}' => {
                flow -= 1;
                i += 1;
                at_start = false;
            }
            b',' if flow > 0 => {
                i += 1;
                at_start = true;
            }
            b':' if ws(at(i + 1)) || (flow > 0 && b",]}".contains(&at(i + 1))) => {
                i += 1;
                at_start = true;
            }
            b'!' if at_start => {
                let s = i;
                while i < n && !ws(b[i]) && !(flow > 0 && b",]}".contains(&b[i])) {
                    i += 1;
                }
                if &text[s..i] == "!secret" {
                    while i < n && (b[i] == b' ' || b[i] == b'\t') {
                        i += 1;
                    }
                    if at(i) == b'\n' || at(i) == b'#' {
                        let line = text[..s].matches('\n').count() + 1;
                        bail!("line {line}: put a secret's value on the same line as `!secret`");
                    }
                    let end = node_end(b, i, flow > 0, parent);
                    out.push((s, end, parent, flow > 0));
                    i = end;
                    at_start = false;
                }
            }
            b'&' if at_start => {
                while i < n && !ws(b[i]) {
                    i += 1;
                }
            }
            b'"' | b'\'' | b'|' | b'>' if at_start => {
                i = node_end(b, i, flow > 0, parent);
                at_start = false;
            }
            _ => {
                // A plain scalar: up to `: `, ` #`, the end of the line (or, in a flow, `,]}`).
                let key_col = i - line_start;
                let s = i;
                i = plain_end(b, i, flow > 0);
                if at(i) == b':' && i > s {
                    parent = key_col;
                }
                at_start = false;
            }
        }
    }
    Ok(out)
}

/// Where a plain scalar starting at `i` ends.
fn plain_end(b: &[u8], mut i: usize, flow: bool) -> usize {
    let n = b.len();
    let ws = |c: u8| c == b' ' || c == b'\t' || c == b'\n' || c == b'\r';
    let mut end = i;
    while i < n && b[i] != b'\n' {
        let c = b[i];
        if c == b':' && (i + 1 == n || ws(b[i + 1]) || (flow && b",]}".contains(&b[i + 1]))) {
            break;
        }
        if c == b'#' && i > 0 && ws(b[i - 1]) {
            break;
        }
        if flow && b",[]{}".contains(&c) {
            break;
        }
        i += 1;
        if !ws(c) {
            end = i;
        }
    }
    end
}

/// Where the scalar starting at `i` ends: quoted, block (`|`, `>`: its
/// lines deeper than `parent`) or plain.
fn node_end(b: &[u8], i: usize, flow: bool, parent: usize) -> usize {
    let n = b.len();
    match b[i] {
        b'"' => {
            let mut j = i + 1;
            while j < n {
                match b[j] {
                    b'\\' => j += 2,
                    b'"' => return j + 1,
                    _ => j += 1,
                }
            }
            n
        }
        b'\'' => {
            let mut j = i + 1;
            while j < n {
                if b[j] == b'\'' {
                    if j + 1 < n && b[j + 1] == b'\'' {
                        j += 2;
                        continue;
                    }
                    return j + 1;
                }
                j += 1;
            }
            n
        }
        b'|' | b'>' if !flow => {
            // The header line, then lines deeper than `parent` (or blank).
            let mut j = i;
            while j < n && b[j] != b'\n' {
                j += 1;
            }
            let mut end = j;
            while j < n {
                let ls = j + 1;
                let mut k = ls;
                while k < n && b[k] == b' ' {
                    k += 1;
                }
                let mut le = k;
                while le < n && b[le] != b'\n' {
                    le += 1;
                }
                let blank = b[k..le].iter().all(|c| *c == b' ' || *c == b'\t' || *c == b'\r');
                if ls >= n || (!blank && k - ls <= parent) {
                    break;
                }
                if !blank {
                    end = le;
                }
                j = le;
            }
            end
        }
        _ => plain_end(b, i, flow),
    }
}

pub(crate) fn key_text(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

/// A secret's value as text.
fn secret_text(v: &Value) -> Result<String> {
    Ok(match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => bail!("it's empty"),
        _ => bail!("a secret is one value (text), not a list or a map"),
    })
}

/// Every `!secret` in the parsed file, in order: where it is, and its value.
fn walk(v: &Value, path: &mut Vec<String>, out: &mut Vec<(String, Result<String>)>) {
    match v {
        Value::Tagged(t) if t.tag == "secret" => out.push((path.join("."), secret_text(&t.value))),
        Value::Tagged(t) => walk(&t.value, path, out),
        Value::Mapping(m) => {
            for (k, v) in m {
                path.push(key_text(k));
                walk(v, path, out);
                path.pop();
            }
        }
        Value::Sequence(s) => {
            for (i, v) in s.iter().enumerate() {
                // `- name:` with its setup: by its name, as it's read.
                if v.as_mapping().is_some_and(|m| m.len() == 1) {
                    walk(v, path, out);
                    continue;
                }
                path.push(i.to_string());
                walk(v, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// The secrets in a file's text: where each is (in the text and in the
/// YAML) and its value. The text is read twice, by the YAML parser and by
/// a scan for where the values are, and both must agree.
pub fn find(text: &str) -> Result<Vec<Found>> {
    if !text.contains("!secret") {
        return Ok(Vec::new());
    }
    let value: Value = serde_yaml::from_str(text)?;
    let mut parsed = Vec::new();
    walk(&value, &mut Vec::new(), &mut parsed);
    let spans = spans(text)?;
    ensure!(spans.len() == parsed.len(), "found {} `!secret` values, where YAML sees {}; write each as `key: !secret value`, on one line", spans.len(), parsed.len());
    let mut out = Vec::new();
    for ((start, end, col, flow), (path, value)) in spans.into_iter().zip(parsed) {
        let line = text[..start].matches('\n').count() + 1;
        let value = value.map_err(|e| anyhow!("line {line}: {path}: {e}"))?;
        // The span alone must read as the same secret.
        let alone: Value = serde_yaml::from_str(&format!("{}\n", &text[start..end])).map_err(|e| anyhow!("line {line}: {path}: can't tell where the secret ends: {e}"))?;
        let same = matches!(&alone, Value::Tagged(t) if t.tag == "secret" && secret_text(&t.value).ok().as_ref() == Some(&value));
        ensure!(same, "line {line}: {path}: can't tell where the secret ends; write it on one line, quoted");
        out.push(Found { start, end, line, col, flow, sealed: is_sealed(&value), path, value });
    }
    Ok(out)
}

/// `text` with each found secret replaced by what `with` gives (None: as it is).
fn replace(text: &str, found: &[Found], mut with: impl FnMut(&Found) -> Result<Option<String>>) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for f in found {
        out += &text[at..f.start];
        match with(f)? {
            Some(new) => out += &new,
            None => out += &text[f.start..f.end],
        }
        at = f.end;
    }
    out += &text[at..];
    Ok(out)
}

/// A secret written plainly, for editing: quoted (or a block, for several lines).
fn plain_form(f: &Found, plain: &str) -> String {
    let lines: Vec<&str> = plain.lines().collect();
    let block = !f.flow
        && plain.contains('\n')
        && !plain.ends_with("\n\n")
        && lines.iter().all(|l| !l.starts_with([' ', '\t']) && !l.contains('\r'))
        && !lines.is_empty();
    if block {
        let pad = " ".repeat(f.col + 2);
        let chomp = if plain.ends_with('\n') { "|" } else { "|-" };
        let body: Vec<String> = lines.iter().map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") }).collect();
        format!("!secret {chomp}\n{}", body.join("\n"))
    } else {
        format!("!secret {}", serde_json::to_string(plain).unwrap_or_default())
    }
}

/// Check that `new` has the same secrets as `old`, at the same places, all sealed.
fn same_places(old: &[Found], new: &str) -> Result<()> {
    let now = find(new)?;
    ensure!(now.len() == old.len() && now.iter().zip(old).all(|(a, b)| a.path == b.path), "the secrets moved while sealing (a bug); nothing was written");
    ensure!(now.iter().all(|f| f.sealed), "a secret didn't seal (a bug); nothing was written");
    Ok(())
}

/// Seal every plain secret in `text` for `recipients`. `reuse` gives the
/// ciphertext for plain values that were sealed before (so an edit doesn't
/// change the lines it didn't touch). Returns the new text and how many it sealed.
fn seal_text(text: &str, recipients: &[(String, String)], reuse: &BTreeMap<String, String>) -> Result<(String, usize)> {
    let found = find(text)?;
    let mut n = 0;
    let new = replace(text, &found, |f| {
        if f.sealed {
            return Ok(None);
        }
        n += 1;
        let ct = match reuse.get(&f.value) {
            Some(ct) => ct.clone(),
            None => encrypt(f.value.as_bytes(), recipients)?,
        };
        Ok(Some(format!("!secret \"{ct}\"")))
    })?;
    if n > 0 {
        same_places(&found, &new)?;
    }
    Ok((new, n))
}

/// A file's new text with its plain secrets sealed, before it's written,
/// so they never touch the disk: for the `.nami-recipients` over `file`
/// (made with your key if there's none). Returns how many it sealed.
pub fn seal_new_text(text: &str, file: &Path) -> Result<(String, usize)> {
    if find(text)?.iter().all(|f| f.sealed) {
        return Ok((text.to_string(), 0));
    }
    let (_, recipients) = recipients_or_make(file)?;
    seal_text(text, &recipients, &BTreeMap::new())
}

/// The `.nami-recipients` for the files in `dir`: the one over it, or a
/// new one in it with your key.
pub fn recipients_for(dir: &Path) -> Result<PathBuf> {
    Ok(recipients_or_make(dir)?.0)
}

/// `text` with its plain secrets hidden, for showing it.
pub fn hide_plain(text: &str) -> Result<String> {
    let found = find(text)?;
    replace(text, &found, |f| Ok((!f.sealed).then(|| "!secret \"<hidden>\"".to_string())))
}

/// `text` with every secret opened (for editing), and each one's
/// ciphertext by its plain value.
fn unseal_text(text: &str, keys: &Keys, file: &Path) -> Result<(String, BTreeMap<String, String>)> {
    let found = find(text)?;
    let mut reuse = BTreeMap::new();
    let new = replace(text, &found, |f| {
        if !f.sealed {
            return Ok(None);
        }
        let plain = keys.decrypt_text(&f.value).map_err(|e| cant_open(file, f, e))?;
        reuse.insert(plain.clone(), f.value.clone());
        Ok(Some(plain_form(f, &plain)))
    })?;
    Ok((new, reuse))
}

/// Every secret in `text` sealed again, for `recipients`.
fn reseal_text(text: &str, keys: &Keys, file: &Path, recipients: &[(String, String)]) -> Result<String> {
    let found = find(text)?;
    let new = replace(text, &found, |f| {
        let plain = if f.sealed { keys.decrypt(&f.value).map_err(|e| cant_open(file, f, e))? } else { f.value.clone().into_bytes() };
        Ok(Some(format!("!secret \"{}\"", encrypt(&plain, recipients)?)))
    })?;
    same_places(&found, &new)?;
    Ok(new)
}

fn cant_open(file: &Path, f: &Found, e: anyhow::Error) -> anyhow::Error {
    anyhow!("{}:{}: can't open the secret {}: {e:#}; {}", file.display(), f.line, f.path, access_hint())
}

/// Turn each `!secret` in a loaded file into a marked string, checking
/// it's sealed and somewhere secrets may go.
pub fn mark(value: &mut Value, name: &str, kind: Kind) -> Result<()> {
    fn go(v: &mut Value, path: &mut Vec<String>, name: &str, kind: Kind) -> Result<()> {
        match v {
            Value::Tagged(t) if t.tag == "secret" => {
                let at = path.join(".");
                let text = secret_text(&t.value).map_err(|e| anyhow!("{name}: {at}: {e}"))?;
                ensure!(is_sealed(&text), "{name}: {at}: this secret isn't sealed yet; run `nami secret seal {name}`");
                ensure!(kind.allows(path), "{name}: {at}: a secret can't go here; {}", kind.places());
                *v = Value::String(format!("{MARK}{text}"));
            }
            Value::Tagged(t) => go(&mut t.value, path, name, kind)?,
            Value::Mapping(m) => {
                for (k, v) in m.iter_mut() {
                    path.push(key_text(k));
                    go(v, path, name, kind)?;
                    path.pop();
                }
            }
            Value::Sequence(s) => {
                for (i, v) in s.iter_mut().enumerate() {
                    if v.as_mapping().is_some_and(|m| m.len() == 1) {
                        go(v, path, name, kind)?;
                        continue;
                    }
                    path.push(i.to_string());
                    go(v, path, name, kind)?;
                    path.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }
    go(value, &mut Vec::new(), name, kind)
}

/// Which file a secret is in, for where secrets may go.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// A system or user file (and what they include).
    Nami,
    Project,
}

impl Kind {
    fn patterns(self) -> &'static [&'static [&'static str]] {
        match self {
            Kind::Nami => &[&["env", "*"], &["files", "*", "**"], &["wifi", "*", "password"]],
            Kind::Project => &[&["env", "*"], &["services", "*", "env", "*"], &["services", "*", "exports", "*"]],
        }
    }

    /// Whether a secret may be at `path` in a file.
    fn allows(self, path: &[String]) -> bool {
        fn matches(p: &[&str], path: &[String]) -> bool {
            match (p.first(), path.first()) {
                (Some(&"**"), _) => true,
                (Some(_), None) | (None, Some(_)) => false,
                (None, None) => true,
                (Some(&"*"), Some(_)) => matches(&p[1..], &path[1..]),
                (Some(x), Some(y)) => x == y && matches(&p[1..], &path[1..]),
            }
        }
        self.patterns().iter().any(|p| matches(p, path))
    }

    fn places(self) -> &'static str {
        match self {
            Kind::Nami => "secrets go in env: and files: (yours in ~/, or the computer's under /etc or /run on a computer nami made), and a Wi-Fi network's password (wifi:)",
            Kind::Project => "secrets go in env:, and a service's env: or exports:",
        }
    }
}

// --- Commands -----------------------------------------------------------------

fn write_keeping_mode(path: &Path, text: &str) -> Result<()> {
    let mode = fs::metadata(path).map(|m| m.permissions().mode()).unwrap_or(0o644);
    crate::store::write_atomic(path, text.as_bytes())?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))?;
    Ok(())
}

/// The nami files in a repo (or folder), for commands given none: those
/// with a `!secret`, not in .git or a project's .nami, nor under another
/// `.nami-recipients`.
fn nami_files(top: &Path) -> Vec<PathBuf> {
    fn go(dir: &Path, top: &Path, out: &mut Vec<PathBuf>) {
        if dir != top && crate::oldnames::is_file(&dir.join(RECIPIENTS)) {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() && !matches!(name.as_str(), ".git" | ".nami" | "node_modules" | "target") {
                go(&e.path(), top, out);
            } else if ty.is_file() && name.ends_with(".nami") && fs::read_to_string(e.path()).is_ok_and(|t| t.contains("!secret")) {
                out.push(e.path());
            }
        }
    }
    let mut out = Vec::new();
    go(top, top, &mut out);
    out
}

/// The top of the repo you're in (or this folder).
fn here() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    if let Some(r) = recipients_file(&cwd) {
        return Ok(r.parent().unwrap().to_path_buf());
    }
    Ok(new_recipients_file(&cwd).parent().unwrap().to_path_buf())
}

/// `nami secret seal [files]`: encrypt the plain `!secret` values.
pub fn seal(files: &[PathBuf]) -> Result<()> {
    let files = if files.is_empty() { nami_files(&here()?) } else { files.to_vec() };
    let mut total = 0;
    for file in &files {
        let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        let found = find(&text).with_context(|| file.display().to_string())?;
        if found.iter().all(|f| f.sealed) {
            continue;
        }
        let (_, recipients) = recipients_or_make(file)?;
        let (new, n) = seal_text(&text, &recipients, &BTreeMap::new()).with_context(|| file.display().to_string())?;
        write_keeping_mode(file, &new)?;
        eprintln!("{}", crate::style::err().line(&format!("✓ {}: sealed {n} secret{} for {}", file.display(), if n == 1 { "" } else { "s" }, names(&recipients))));
        total += n;
    }
    if total == 0 {
        eprintln!("{}", crate::style::err().line("✓ Nothing to seal: every secret is sealed"));
    }
    Ok(())
}

fn names(r: &[(String, String)]) -> String {
    r.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
}

/// `nami secret edit <file>`: the file in $EDITOR with its secrets opened;
/// sealed again after. The opened copy is in memory ($XDG_RUNTIME_DIR or
/// /dev/shm), private, and gone after.
pub fn edit(file: &Path) -> Result<()> {
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let keys = Keys::mine();
    let (open, reuse) = unseal_text(&text, &keys, file)?;
    let mut recipients = Vec::new();
    let dir = [std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from), Some(PathBuf::from("/dev/shm"))]
        .into_iter()
        .flatten()
        .find(|d| d.is_dir() && fs::metadata(d).is_ok_and(|m| !m.permissions().readonly()))
        .unwrap_or_else(std::env::temp_dir);
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file.nami".into());
    let tmp = dir.join(format!("nami-secret-{}-{name}", std::process::id()));
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_else(|_| "nano".into());
    let mut current = open.clone();
    let result = loop {
        let _ = fs::remove_file(&tmp);
        {
            let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.write_all(current.as_bytes())?;
        }
        let status = std::process::Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("sh").arg(&tmp).status();
        let edited = fs::read_to_string(&tmp);
        wipe(&tmp);
        if !status.is_ok_and(|s| s.success()) {
            break Err(anyhow!("the editor ({editor}) didn't finish cleanly; nothing was changed"));
        }
        let edited = edited?;
        if edited == open {
            eprintln!("{}: nothing changed", file.display());
            break Ok(None);
        }
        if recipients.is_empty() && edited.contains("!secret") {
            recipients = recipients_or_make(file)?.1;
        }
        match seal_text(&edited, &recipients, &reuse) {
            Ok((new, _)) => break Ok(Some(new)),
            Err(e) if crate::errors::may_ask() => {
                eprintln!("{e:#}\nPress Enter to edit it again (Ctrl-C leaves the file as it was).");
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                current = edited;
            }
            Err(e) => break Err(e.context("nothing was changed")),
        }
    };
    if let Some(new) = result? {
        write_keeping_mode(file, &new)?;
        if recipients.is_empty() {
            eprintln!("{}", crate::style::err().line(&format!("✓ {}: saved", file.display())));
        } else {
            eprintln!("{}", crate::style::err().line(&format!("✓ {}: saved, its secrets sealed for {}", file.display(), names(&recipients))));
        }
    }
    Ok(())
}

/// Overwrite, then remove.
fn wipe(path: &Path) {
    if let Ok(len) = fs::metadata(path).map(|m| m.len()) {
        let _ = fs::write(path, vec![0u8; len as usize]);
    }
    let _ = fs::remove_file(path);
}

/// The secret at `key` in a file's secrets: a dotted path (`env.STRIPE_KEY`),
/// or the end of one if only one ends so.
fn pick<'a>(found: &'a [Found], key: &str) -> Option<&'a Found> {
    if let Some(f) = found.iter().find(|f| f.path == key) {
        return Some(f);
    }
    let ends: Vec<&Found> = found.iter().filter(|f| f.path.ends_with(&format!(".{key}"))).collect();
    (ends.len() == 1).then(|| ends[0])
}

/// `nami secret show <file> [key]`: print one secret (for scripts), or list them.
pub fn show(file: &Path, key: Option<&str>) -> Result<()> {
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let found = find(&text).with_context(|| file.display().to_string())?;
    let Some(key) = key else {
        for f in &found {
            println!("{}{}", f.path, if f.sealed { "" } else { "  (not sealed yet)" });
        }
        return Ok(());
    };
    let f = pick(&found, key).ok_or_else(|| {
        let all: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
        anyhow!("{}: no secret {key}{}", file.display(), if all.is_empty() { String::new() } else { format!(" (it has: {})", all.join(", ")) })
    })?;
    let plain = if f.sealed { Keys::mine().decrypt(&f.value).map_err(|e| cant_open(file, f, e))? } else { f.value.clone().into_bytes() };
    let mut out = std::io::stdout();
    out.write_all(&plain)?;
    if unsafe { libc::isatty(1) } == 1 && !plain.ends_with(b"\n") {
        out.write_all(b"\n")?;
    }
    Ok(())
}

/// `nami secret allow <name> <key>` (or `--remove`): change who can open
/// the repo's secrets, and seal them all again for whoever that is now.
/// Returns the files it changed (to commit).
pub fn allow(name: &str, key: Option<&str>, remove: bool, at: Option<&Path>) -> Result<Vec<PathBuf>> {
    ensure!(!name.is_empty() && !name.contains(char::is_whitespace) && !name.starts_with('#'), "a name is one word, like ada or laptop");
    let start = match at {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let key = match key {
        Some(k) if Path::new(k).is_file() => fs::read_to_string(k)?.split_whitespace().take(2).collect::<Vec<_>>().join(" "),
        Some(k) => k.trim().to_string(),
        None if remove => String::new(),
        None => bail!("whose key? `nami secret allow {name} <public key>` (they get theirs with `nami secret key`)"),
    };
    if !remove {
        recipient(&key)?;
    }
    let (file, old) = match recipients_of(&start)? {
        Some(r) => r,
        None => recipients_or_make(&start)?,
    };
    let mut list = old.clone();
    let removed: Vec<(String, String)>;
    if remove {
        ensure!(list.iter().any(|(n, _)| n == name), "{} has no {name} (it has: {})", file.display(), names(&list));
        removed = list.iter().filter(|(n, _)| n == name).cloned().collect();
        list.retain(|(n, _)| n != name);
        ensure!(!list.is_empty(), "{name} is the last one in {}; then nobody could open the secrets", file.display());
    } else {
        removed = Vec::new();
        match list.iter_mut().find(|(n, _)| n == name) {
            Some((_, k)) if *k == key => {
                eprintln!("{name} is already in {} with that key", file.display());
                return Ok(Vec::new());
            }
            Some((_, k)) => {
                eprintln!("{name}: a new key (the old one can't open what's sealed from now on)");
                *k = key.clone();
            }
            None => list.push((name.to_string(), key.clone())),
        }
    }
    let top = file.parent().unwrap().to_path_buf();
    let files = nami_files(&top);
    // Everything opened and sealed again first; only then written.
    let keys = Keys::mine();
    let mut new = Vec::new();
    let mut were = Vec::new();
    for f in &files {
        let text = fs::read_to_string(f)?;
        for s in find(&text).with_context(|| f.display().to_string())? {
            were.push(format!("{}:{}: {}", f.display(), s.line, s.path));
        }
        new.push((f, reseal_text(&text, &keys, f, &list)?));
    }
    let old_text = fs::read_to_string(&file).unwrap_or_default();
    write_keeping_mode(&file, &recipients_text(&old_text, &list))?;
    for (f, text) in &new {
        write_keeping_mode(f, text)?;
    }
    let n = were.len();
    eprintln!("{}: {}; {n} secret{} in {} file{} sealed again for {}", file.display(), if remove { format!("removed {name}") } else { format!("{name} can open the secrets") }, if n == 1 { "" } else { "s" }, new.len(), if new.len() == 1 { "" } else { "s" }, names(&list));
    if !removed.is_empty() && n > 0 {
        eprintln!("{name} could open these, and git keeps the old versions: change them where they come from (a new password, a new token), then `nami secret edit`:");
        for w in &were {
            println!("  {w}");
        }
    }
    let mut changed = vec![file.clone()];
    changed.extend(new.iter().map(|(f, _)| f.to_path_buf()));
    Ok(changed)
}

/// `nami secret key`: your public key, for someone to `nami secret allow`.
pub fn key() -> Result<()> {
    println!("{}", my_public_key(true)?.expect("made"));
    Ok(())
}

/// Secrets not sealed yet, in `files` (or what git is about to commit).
/// Returns the problems, as `file:line: path` lines.
pub fn check(files: &[PathBuf], staged: bool) -> Result<Vec<String>> {
    let mut texts: Vec<(String, String)> = Vec::new();
    if staged {
        let out = std::process::Command::new("git").args(["diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z"]).output().context("running git")?;
        ensure!(out.status.success(), "git diff failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        for name in out.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()) {
            let name = String::from_utf8_lossy(name).into_owned();
            if !name.ends_with(".nami") {
                continue;
            }
            let show = std::process::Command::new("git").arg("show").arg(format!(":{name}")).output()?;
            texts.push((name, String::from_utf8_lossy(&show.stdout).into_owned()));
        }
    } else {
        let files = if files.is_empty() { nami_files(&here()?) } else { files.to_vec() };
        for f in files {
            texts.push((f.display().to_string(), fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?));
        }
    }
    let mut problems = Vec::new();
    for (name, text) in texts {
        if !text.contains("!secret") {
            continue;
        }
        match find(&text) {
            Ok(found) => problems.extend(found.iter().filter(|f| !f.sealed).map(|f| format!("{name}:{}: {} isn't sealed", f.line, f.path))),
            // Not YAML yet: nami says so elsewhere; but a plain secret might be in it.
            Err(e) => problems.push(format!("{name}: {e:#}")),
        }
    }
    Ok(problems)
}

/// `nami secret hook`: a git pre-commit hook that refuses unsealed secrets.
pub fn hook() -> Result<()> {
    let out = std::process::Command::new("git").args(["rev-parse", "--git-path", "hooks"]).output().context("running git")?;
    ensure!(out.status.success(), "not in a git repo");
    let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let dir = if dir.is_absolute() { dir } else { std::env::current_dir()?.join(dir) };
    let path = dir.join("pre-commit");
    let exe = std::env::current_exe()?.display().to_string();
    let line = "nami secret check --staged";
    let script = format!("#!/bin/sh\n# Refuses commits with secrets that aren't sealed (`nami secret hook`).\nnami=$(command -v nami || echo '{exe}')\nexec \"$nami\" secret check --staged\n");
    match fs::read_to_string(&path) {
        Ok(t) if t.contains("secret check --staged") => {
            eprintln!("{} already checks for unsealed secrets", path.display());
            return Ok(());
        }
        Ok(_) => bail!("{} is there already; add this line to it: {line}", path.display()),
        Err(_) => {}
    }
    fs::create_dir_all(&dir)?;
    fs::write(&path, script)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    eprintln!("installed {}: commits with unsealed secrets are refused", path.display());
    Ok(())
}

/// Write a file with a secret in it, for its owner alone (or `mode`).
pub fn install_file(dest: &Path, data: &[u8], mode: u32) -> Result<()> {
    install_file_owned(dest, data, mode, None)
}

/// [`install_file`], owned by `owner` (uid, gid).
pub fn install_file_owned(dest: &Path, data: &[u8], mode: u32, owner: Option<(u32, u32)>) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_file_name(format!(".{}.nami-tmp", dest.file_name().and_then(|n| n.to_str()).unwrap_or("secret")));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    if let Some((uid, gid)) = owner {
        std::os::unix::fs::fchown(&f, Some(uid), Some(gid)).with_context(|| format!("giving {} to {uid}:{gid}", dest.display()))?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    drop(f);
    fs::rename(&tmp, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> (age::x25519::Identity, Vec<(String, String)>) {
        let id = age::x25519::Identity::generate();
        let list = vec![("me".to_string(), id.to_public().to_string())];
        (id, list)
    }

    fn keys(ids: Vec<age::x25519::Identity>) -> Keys {
        Keys { names: ids.iter().map(|_| "test".to_string()).collect(), ids: ids.into_iter().map(|i| Box::new(i) as Box<dyn age::Identity>).collect() }
    }

    const FILE: &str = r#"# my setup
wifi:
  home: { password: !secret hunter2, ssid: "Home Net" }   # the router's
env:
  # Stripe, for testing
  STRIPE_KEY: !secret "sk_test_123 # not a comment"
  PLAIN: "it's !secret not"
  QUOTED: !secret 'it''s'
list:
  - !secret item
key:
  nested: !secret |
    line one
    line two
  after: 1
- not: yaml
"#;

    #[test]
    fn sealing_keeps_everything_else() {
        let text = FILE.replace("- not: yaml\n", "");
        let (id, list) = me();
        let found = find(&text).unwrap();
        let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["wifi.home.password", "env.STRIPE_KEY", "env.QUOTED", "list.0", "key.nested"]);
        assert_eq!(found[1].value, "sk_test_123 # not a comment");
        assert_eq!(found[2].value, "it's");
        assert_eq!(found[4].value, "line one\nline two\n");
        assert_eq!(found[4].line, 12);
        let (sealed, n) = seal_text(&text, &list, &BTreeMap::new()).unwrap();
        assert_eq!(n, 5);
        // Everything but the secrets, byte for byte.
        let strip = |t: &str| t.lines().filter(|l| !l.contains("!secret") && !l.starts_with("    line")).collect::<Vec<_>>().join("\n");
        assert_eq!(strip(&sealed), strip(&text));
        assert!(sealed.contains("  home: { password: !secret \"YWdl") && sealed.contains("\", ssid: \"Home Net\" }   # the router's\n"));
        assert!(sealed.contains("  after: 1\n") && !sealed.contains("line one"));
        assert!(!sealed.contains("hunter2"));
        // Sealing again changes nothing.
        assert_eq!(seal_text(&sealed, &list, &BTreeMap::new()).unwrap(), (sealed.clone(), 0));
        // And they open again, the same.
        let k = keys(vec![id]);
        let found = find(&sealed).unwrap();
        assert!(found.iter().all(|f| f.sealed));
        let opened: Vec<String> = found.iter().map(|f| k.decrypt_text(&f.value).unwrap()).collect();
        assert_eq!(opened, ["hunter2", "sk_test_123 # not a comment", "it's", "item", "line one\nline two\n"]);
    }

    #[test]
    fn round_trip_through_edit() {
        let text = FILE.replace("- not: yaml\n", "");
        let (id, list) = me();
        let (sealed, _) = seal_text(&text, &list, &BTreeMap::new()).unwrap();
        let k = keys(vec![id]);
        let (open, reuse) = unseal_text(&sealed, &k, Path::new("t.nami")).unwrap();
        assert!(open.contains("password: !secret \"hunter2\", ssid") && open.contains("  nested: !secret |\n    line one\n    line two\n  after: 1"));
        // Unchanged: sealed back to exactly the same text.
        assert_eq!(seal_text(&open, &list, &reuse).unwrap().0, sealed);
        // One changed: only its line differs.
        let changed = seal_text(&open.replace("\"hunter2\"", "\"hunter3\""), &list, &reuse).unwrap().0;
        let diff: Vec<(&str, &str)> = sealed.lines().zip(changed.lines()).filter(|(a, b)| a != b).collect();
        assert_eq!(diff.len(), 1);
        assert!(diff[0].0.starts_with("  home:"));
    }

    #[test]
    fn recipients_added_and_removed() {
        let text = "env:\n  A: !secret one\n";
        let (a, mut list) = me();
        let b = age::x25519::Identity::generate();
        let (sealed, _) = seal_text(text, &list, &BTreeMap::new()).unwrap();
        let ka = keys(vec![a.clone()]);
        let kb = keys(vec![b.clone()]);
        let f = &find(&sealed).unwrap()[0];
        assert!(kb.decrypt(&f.value).is_err());
        // b allowed: both open it.
        list.push(("b".into(), b.to_public().to_string()));
        let both = reseal_text(&sealed, &ka, Path::new("t"), &list).unwrap();
        let f = &find(&both).unwrap()[0];
        assert_eq!(ka.decrypt_text(&f.value).unwrap(), "one");
        assert_eq!(kb.decrypt_text(&f.value).unwrap(), "one");
        // a removed: only b.
        list.remove(0);
        let only_b = reseal_text(&both, &kb, Path::new("t"), &list).unwrap();
        let f = &find(&only_b).unwrap()[0];
        assert!(ka.decrypt(&f.value).unwrap_err().to_string().contains("none of your keys"));
        assert_eq!(kb.decrypt_text(&f.value).unwrap(), "one");
        // Can't open: says which file, line and secret.
        let e = reseal_text(&only_b, &ka, Path::new("me.nami"), &list).unwrap_err().to_string();
        assert!(e.contains("me.nami:2: can't open the secret env.A") && e.contains("nami secret allow"), "{e}");
    }

    #[test]
    fn recipients_file_keeps_its_comments() {
        let old = "# who\nada age1abc # laptop\n\nold age1def\n";
        let list = vec![("ada".to_string(), "age1xyz".to_string()), ("new".to_string(), "age1new".to_string())];
        assert_eq!(recipients_text(old, &list), "# who\nada age1xyz\n\nnew age1new\n");
        let id = age::x25519::Identity::generate().to_public().to_string();
        let parsed = parse_recipients(&format!("# c\nlaptop {id}\nada ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHsKLqeplhpW+uObz5dvMgjz1OxfM/XXUB+VHtZ6isGN ada@x\n")).unwrap();
        assert_eq!(parsed[0], ("laptop".to_string(), id));
        assert!(parsed[1].1.starts_with("ssh-ed25519 AAAA"));
        assert!(parse_recipients("ada nope\n").is_err());
    }

    #[test]
    fn marked_only_where_secrets_go() {
        let (_, list) = me();
        let (sealed, _) = seal_text("env:\n  T: !secret tok\npackages: [jq]\n", &list, &BTreeMap::new()).unwrap();
        let mut v: Value = serde_yaml::from_str(&sealed).unwrap();
        mark(&mut v, "user.nami", Kind::Nami).unwrap();
        assert!(marked(v["env"]["T"].as_str().unwrap()).is_some());
        let mut v: Value = serde_yaml::from_str(&sealed.replace("env:", "environment:")).unwrap();
        assert!(mark(&mut v, "user.nami", Kind::Nami).unwrap_err().to_string().contains("can't go here"));
        let mut v: Value = serde_yaml::from_str("env: { A: !secret plain }").unwrap();
        assert!(mark(&mut v, "project.nami", Kind::Project).unwrap_err().to_string().contains("nami secret seal project.nami"));
    }

    #[test]
    fn unclear_secrets_are_refused() {
        assert!(find("a: !secret\n  b\n").is_err());
        assert!(find("a: !secret {x: 1}\n").is_err());
        // An alias: the parser sees two, the text one.
        assert!(find("a: &x !secret v\nb: *x\n").is_err());
    }
}
