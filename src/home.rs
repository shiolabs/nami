//! A nami file's `~/` files, env, aliases and shell lines: your dotfiles and
//! program settings, as YAML.
//!
//! ```yaml
//! env: { EDITOR: nvim }
//! aliases: { ll: ls -la }
//! shell:                       # lines for interactive shells' startup
//!   zsh: |
//!     eval "$(starship init zsh)"
//! files:
//!   ~/.gitconfig:                   # nested YAML -> the file's own format
//!     user: { name: Ada, email: ada@example.org }
//!   ~/.vimrc: |                     # text -> the file, as is
//!     set number
//!   ~/.config/nvim: { source: ./files/nvim }   # a file or dir from your repo
//! ```
//!
//! The format follows the file name: .json, .toml, .yaml/.yml, .ini/.conf/
//! .cfg and gitconfig (INI with `[remote "origin"]` subsections), .env. The
//! rendered files become one store path per generation; `~/<file>` is a link
//! into the active profile, so `nami undo` brings old configs back too. A
//! `source:` in your home (a config repo's checkout) is linked to where it
//! is instead: edit it and it's live, and git sees the change.
//! A file already in the way is kept as `<file>.nami-backup`, and restored
//! when nami stops managing that path.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};

use crate::store::{write_atomic, Store};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Home {
    /// Environment variables for shells and the graphical session; a value
    /// can be a secret (`!secret ...`).
    #[serde(default, deserialize_with = "crate::yaml_text::text_map")]
    pub env: BTreeMap<String, String>,
    /// Shell aliases (bash, zsh, fish).
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    /// `bash`/`zsh`/`fish` -> lines run at the end of that shell's startup
    /// (interactive shells only), like the end of your ~/.zshrc.
    #[serde(default)]
    pub shell: BTreeMap<String, String>,
    /// `zsh` or `fish`: the shell your terminals open. Their interactive
    /// bash hands over to it (no root, no chsh); while it isn't installed,
    /// bash stays.
    #[serde(default, rename = "login-shell")]
    pub login_shell: Option<String>,
    /// `path relative to ~` -> text, nested YAML, or `{ source: ./path }`.
    #[serde(default)]
    pub files: Mapping,
}

/// A file with a secret in it: written into ~ on activation, private.
/// `content` keeps the secrets sealed (see [`crate::secrets`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecretFile {
    pub rel: String,
    pub content: Value,
    pub mode: u32,
}

impl SecretFile {
    /// What the file says, its secrets opened.
    pub fn render(&self, keys: &crate::secrets::Keys) -> Result<Vec<u8>> {
        render_secret(&self.rel, &keys.reveal_value(&self.content)?)
    }
}

/// A file's content with its secrets opened, in the file's format.
pub fn render_secret(rel: &str, content: &Value) -> Result<Vec<u8>> {
    Ok(match content {
        Value::String(s) => s.clone().into_bytes(),
        Value::Mapping(_) | Value::Sequence(_) => render(rel, content)?.into_bytes(),
        _ => bail!("expected text or settings"),
    })
}

/// A home.files value with a secret in it: the whole value, or
/// `{ content: ..., mode: "0600" }`.
pub fn secret_file(rel: &str, value: &Value) -> Result<SecretFile> {
    let mut content = value;
    let mut mode = 0o600;
    if let Value::Mapping(m) = value {
        if m.contains_key("content") && crate::etc::is_options(m) {
            crate::etc::check_options(value, true)?;
            content = &m["content"];
            mode = crate::etc::mode_of(m)?.unwrap_or(mode);
        }
        ensure!(!m.contains_key("source") && !m.contains_key("mutable"), "a file with a secret in it is written by nami each time (no source: or mutable:)");
    }
    // Its format must work (with stand-ins for the secrets).
    fn stand_in(v: &Value) -> Value {
        match v {
            Value::String(s) if crate::secrets::marked(s).is_some() => Value::String("secret".into()),
            Value::Mapping(m) => Value::Mapping(m.iter().map(|(k, v)| (k.clone(), stand_in(v))).collect()),
            Value::Sequence(s) => Value::Sequence(s.iter().map(stand_in).collect()),
            other => other.clone(),
        }
    }
    render_secret(rel, &stand_in(content))?;
    Ok(SecretFile { rel: rel.to_string(), content: content.clone(), mode })
}

/// A generation's home: the store path holding the rendered files, and the
/// top-level entries linked into ~.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HomeState {
    pub path: String,
    pub files: Vec<String>,
    /// Of `files`, those copied into ~ (writable) rather than linked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mutable: Vec<String>,
    /// The modes of those that have one of their own (`mode: "0600"`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modes: BTreeMap<String, u32>,
    /// Files and env vars with secrets: kept sealed here, opened on activation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_files: Vec<SecretFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_env: Vec<(String, String)>,
    /// `settings:` from your nami file (desktop settings via gsettings).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: crate::settings::Settings,
}

/// Render `home` (paths in it relative to `base`) and add it to the store.
/// Secrets are only checked here (they must decrypt), never written.
pub fn build(store: &Store, home: &Home, base: &Path, settings: crate::settings::Settings) -> Result<HomeState> {
    let dir = store.cache_dir().join(format!("home-build-{}", std::process::id()));
    if dir.exists() {
        crate::store::remove_tree(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    let result = (|| {
        let (mut files, mut mutable, mut secret_files, mut modes) = (Vec::new(), Vec::new(), Vec::new(), BTreeMap::new());
        let keys = crate::secrets::Keys::mine();
        let home_dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        for (key, value) in &home.files {
            let rel = key.as_str().ok_or_else(|| anyhow!("files: each is a path, like ~/.config/app/settings.toml"))?;
            let rel = check_rel(rel)?;
            if crate::secrets::contains(value) {
                let sf = secret_file(&rel, value).with_context(|| format!("files: ~/{rel}"))?;
                // It must open here; it's opened again (in memory) on activation.
                sf.render(&keys).map_err(|e| anyhow!("files: ~/{rel}: can't open its secret: {e:#}; {}", crate::secrets::access_hint()))?;
                secret_files.push(sf);
                continue;
            }
            let dest = dir.join(&rel);
            fs::create_dir_all(dest.parent().unwrap())?;
            let (value, is_mutable, mode) = unwrap_options(value).with_context(|| format!("files: ~/{rel}"))?;
            if let (Some(mode), false) = (mode, is_mutable) {
                // A file with a mode of its own is written in place, private
                // as you say, as one with a secret is.
                let content = match value {
                    Value::Mapping(m) if m.contains_key("source") => {
                        let src = m["source"].as_str().ok_or_else(|| anyhow!("files: ~/{rel}: source: a path"))?;
                        let text = fs::read_to_string(base.join(src)).map_err(|e| anyhow!("files: ~/{rel}: mode: needs a text file from source: ({src}: {e})"))?;
                        Value::String(text)
                    }
                    v => v.clone(),
                };
                render_secret(&rel, &content).with_context(|| format!("files: ~/{rel}"))?;
                secret_files.push(SecretFile { rel, content, mode });
                continue;
            }
            if let Some(mode) = mode.filter(|_| is_mutable) {
                modes.insert(rel.clone(), mode);
            }
            if let Some(src) = live_source(value, base, &home_dir).filter(|_| !is_mutable) {
                // Yours, in your home (a config repo's checkout): linked, so
                // what you change there is live, and git sees it.
                symlink(&src, &dest).with_context(|| format!("files: ~/{rel}"))?;
                files.push(rel);
                continue;
            }
            write_entry(&dest, &rel, value, base).with_context(|| format!("files: ~/{rel}"))?;
            if is_mutable {
                mutable.push(rel.clone());
            }
            files.push(rel);
        }
        for (rel, text) in env_files(home) {
            ensure!(!files.contains(&rel), "files: ~/{rel} clashes with what `env:`/`aliases:`/`shell:` generate");
            fs::create_dir_all(dir.join(&rel).parent().unwrap())?;
            fs::write(dir.join(&rel), text)?;
            files.push(rel);
        }
        let mut secret_env = Vec::new();
        for (k, v) in &home.env {
            if crate::secrets::marked(v).is_some() {
                env_secret(&keys, k, v)?;
                secret_env.push((k.clone(), v.clone()));
            }
        }
        files.sort();
        mutable.sort();
        let path = store.add_path(&dir, "home-files")?;
        Ok(HomeState { path, files, mutable, modes, secret_files, secret_env, settings })
    })();
    crate::store::remove_tree(&dir)?;
    result
}

/// No absolute paths, no `..`: everything stays inside ~.
pub fn check_rel(rel: &str) -> Result<String> {
    let p = Path::new(rel);
    ensure!(!rel.is_empty() && p.is_relative(), "files: ~/{rel}: must be a path relative to your home directory");
    for c in p.components() {
        ensure!(matches!(c, Component::Normal(_)), "files: ~/{rel}: no '..' or '.' allowed");
    }
    Ok(p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

/// A file's options in your home, as every file has them (see
/// [`crate::etc::OPTIONS`]: only with `content:` or `source:`, so a
/// settings file may have a `mode:` of its own): the content to write, the
/// whole value for a `source:`, whether it's mutable, and its mode.
pub fn unwrap_options(value: &Value) -> Result<(&Value, bool, Option<u32>)> {
    let Value::Mapping(m) = value else { return Ok((value, false, None)) };
    if !crate::etc::is_options(m) {
        return Ok((value, false, None));
    }
    crate::etc::check_options(value, true)?;
    let mutable = m.get("mutable").and_then(Value::as_bool).unwrap_or(false);
    let mode = crate::etc::mode_of(m)?;
    match m.get("content") {
        Some(c) => Ok((c, mutable, mode)),
        // `{ source: ./x, ... }`: handed on as its own entry.
        None => Ok((value, mutable, mode)),
    }
}

/// A `{ source: ./x }` in your home: its real path, to link to. (A source
/// from elsewhere, like a set nami fetched, is copied.)
fn live_source(value: &Value, base: &Path, home: &Path) -> Option<std::path::PathBuf> {
    let Value::Mapping(m) = value else { return None };
    let src = m.get("source")?.as_str().filter(|_| m.len() == 1)?;
    let real = fs::canonicalize(base.join(src)).ok()?;
    let home = fs::canonicalize(home).ok()?;
    (real.starts_with(&home) && !real.starts_with(home.join(".cache"))).then_some(real)
}

pub fn write_entry(dest: &Path, rel: &str, value: &Value, base: &Path) -> Result<()> {
    match value {
        Value::String(text) => Ok(fs::write(dest, text)?),
        Value::Mapping(m) if m.contains_key("source") && m.keys().all(|k| k.as_str().is_some_and(|k| crate::etc::OPTIONS.contains(&k))) => {
            let src = m["source"].as_str().ok_or_else(|| anyhow!("source must be a path"))?;
            let src = base.join(src);
            copy_tree(&src, dest).with_context(|| format!("copying {}", src.display()))
        }
        Value::Mapping(_) | Value::Sequence(_) => Ok(fs::write(dest, render(rel, value)?)?),
        Value::Tagged(t) => write_entry(dest, rel, &t.value, base),
        other => bail!("text, settings or {{ source: ... }}, not {}", crate::yaml_text::scalar_text(other).unwrap_or_else(|| "this".into())),
    }
}

fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir_all(dest)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()))?;
        }
    } else if meta.file_type().is_symlink() {
        symlink(fs::read_link(src)?, dest)?;
    } else {
        fs::copy(src, dest)?;
    }
    Ok(())
}

/// Settings -> file contents, in the format the file name implies.
/// A file in a format nami reads and writes itself (JSON, TOML, YAML, INI,
/// gitconfig): written as settings, one way, never as text. (A file kept as
/// it is goes in the repo: `{ source: ./... }`.) `.conf`, `.cfg` and `.env`
/// are many formats under one name, so they may be either.
pub fn settings_only(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    name == ".gitconfig" || name == "gitconfig" || rel.ends_with("git/config") || matches!(ext.as_str(), "json" | "toml" | "yaml" | "yml" | "ini")
}

/// What to say for a file written as text that has to be settings.
pub fn as_settings_hint(rel: &str) -> String {
    format!("{rel} is a format nami writes itself: give its settings as YAML (nested keys), or keep the file in your repo and point at it with {{ source: ./... }}")
}

pub fn render(rel: &str, value: &Value) -> Result<String> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let git = name == ".gitconfig" || name == "gitconfig" || rel.ends_with("git/config");
    match ext.as_str() {
        _ if git => ini(value, true),
        "json" => Ok(serde_json::to_string_pretty(value)? + "\n"),
        "toml" => Ok(toml::to_string_pretty(value).map_err(|e| anyhow!("can't write as TOML: {e}"))?),
        "yaml" | "yml" => Ok(serde_yaml::to_string(value)?),
        "ini" | "conf" | "cfg" | "nmconnection" => ini(value, false),
        "env" => env_file(value),
        _ => bail!("can't tell which format {rel} uses from its name; write its contents as text (`|`) instead"),
    }
}

fn scalar(v: &Value) -> Result<String> {
    Ok(match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        Value::Tagged(t) => scalar(&t.value)?,
        other => bail!("a plain value (text, a number or true/false), not {}", if other.is_mapping() { "a map" } else if other.is_sequence() { "a list" } else { "this" }),
    })
}

/// INI. Maps of maps become git-style subsections: `remote: { origin: {...} }`
/// is `[remote "origin"]`. Lists repeat the key (`[remote "x"] fetch = ...`).
fn ini(value: &Value, git: bool) -> Result<String> {
    let map = value.as_mapping().ok_or_else(|| anyhow!("INI settings must be a map of sections"))?;
    let mut out = String::new();
    let indent = if git { "\t" } else { "" };
    let quote = |s: String| -> String {
        if git && (s.starts_with(' ') || s.ends_with(' ') || s.contains(['#', ';', '"', '\\'])) {
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        } else {
            s
        }
    };
    let emit = |out: &mut String, header: String, body: &Mapping| -> Result<Vec<(String, Mapping)>> {
        let mut subs = Vec::new();
        let mut lines = String::new();
        for (k, v) in body {
            let k = scalar(k)?;
            match v {
                Value::Mapping(m) => subs.push((k, m.clone())),
                Value::Sequence(items) => {
                    for i in items {
                        lines += &format!("{indent}{k} = {}\n", quote(scalar(i)?));
                    }
                }
                v => lines += &format!("{indent}{k} = {}\n", quote(scalar(v)?)),
            }
        }
        if !lines.is_empty() || subs.is_empty() {
            out.push_str(&header);
            out.push_str(&lines);
        }
        Ok(subs)
    };
    // Keys before any section (allowed by many INI readers); a list repeats its key.
    for (k, v) in map {
        match v {
            Value::Mapping(_) => {}
            Value::Sequence(items) => {
                for i in items {
                    out += &format!("{} = {}\n", scalar(k)?, quote(scalar(i)?));
                }
            }
            v => out += &format!("{} = {}\n", scalar(k)?, quote(scalar(v)?)),
        }
    }
    for (section, v) in map {
        let Value::Mapping(body) = v else { continue };
        let section = scalar(section)?;
        let subs = emit(&mut out, format!("[{section}]\n"), body)?;
        for (sub, body) in subs {
            let header = if git { format!("[{section} \"{sub}\"]\n") } else { format!("[{section}.{sub}]\n") };
            let deeper = emit(&mut out, header, &body)?;
            ensure!(deeper.is_empty(), "[{section}] {sub}: INI can't nest that deep");
        }
    }
    Ok(out)
}

fn env_file(value: &Value) -> Result<String> {
    let map = value.as_mapping().ok_or_else(|| anyhow!(".env settings must be a map"))?;
    let mut out = String::new();
    for (k, v) in map {
        let v = scalar(v)?;
        let v = if v.contains(|c: char| c.is_whitespace() || "\"'$#".contains(c)) { format!("\"{}\"", v.replace('"', "\\\"")) } else { v };
        out += &format!("{}={v}\n", scalar(k)?);
    }
    Ok(out)
}

pub const SHELLS: &[&str] = &["bash", "zsh", "fish"];

/// `nami_share <path>...`: where an installed package's `share/<path>` is
/// (your apps first, then the computer's; the first path that exists, as
/// packages move files between versions), so `home.shell` can source a
/// plugin without a store path in it.
fn nami_share_sh() -> String {
    format!(
        r#"nami_share() {{
  for __nami_p in "$@"; do
    for __nami_d in "$HOME/.local/state/nami/profile/share" {}/profiles/default/share /run/current-system/sw/share; do
      if [ -e "$__nami_d/$__nami_p" ]; then printf '%s\n' "$__nami_d/$__nami_p"; unset __nami_d __nami_p; return 0; fi
    done
  done
  unset __nami_d __nami_p; echo "nami_share: no share/$1 in your apps (is its package in your nami file?)" >&2; return 1
}}
"#,
        crate::migrate::here().display()
    )
}

/// A value in single quotes, for sh and fish.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `shell:` -> one file per shell: `~/.config/nami/shell.<bash|zsh>` (a
/// line at the end of ~/.bashrc / ~/.zshrc sources it, see hook_shell_rcs),
/// fish's in conf.d.
fn shell_files(home: &Home) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for (sh, text) in &home.shell {
        let mut text = if text.ends_with('\n') || text.is_empty() { text.clone() } else { format!("{text}\n") };
        // Your aliases again, after these lines: what they load (oh-my-zsh,
        // a distro's bashrc) defines its own `ll` and `ls`, and yours win, as
        // with home-manager.
        if !home.aliases.is_empty() {
            text += "# your aliases, over what the lines above defined\n";
            for (k, v) in &home.aliases {
                text += &if sh == "fish" { format!("alias {k} {}\n", sh_quote(v)) } else { format!("alias {k}={}\n", sh_quote(v)) };
            }
        }
        let head = "# generated by nami from your nami file (home.shell); interactive shells only\n";
        match sh.as_str() {
            "fish" => files.push((".config/fish/conf.d/nami-shell.fish".to_string(), format!("{head}if status is-interactive\n{text}end\n"))),
            _ => files.push((format!(".config/nami/shell.{sh}"), format!("{head}{}{text}", nami_share_sh()))),
        }
    }
    if let Some(sh) = home.login_shell.as_deref().filter(|s| LOGIN_SHELLS.contains(s)) {
        files.push((LOGIN_FILE.to_string(), login_script(sh)));
    }
    files
}

/// What home.login-shell can be: bash is where it starts from.
pub const LOGIN_SHELLS: &[&str] = &["zsh", "fish"];
const LOGIN_FILE: &str = ".config/nami/login.bash";

/// Sourced at the end of ~/.bashrc: an interactive bash becomes `sh`. A
/// bash you start from that shell stays bash (NAMI_LOGIN_SHELL is set), and
/// without `sh` installed nothing happens, so you can always get in.
fn login_script(sh: &str) -> String {
    format!(
        r#"# generated by nami from your nami file (home.login-shell): terminals open {sh}
case $- in
  *i*)
    if [ -z "${{NAMI_LOGIN_SHELL:-}}" ] && __nami_sh=$(command -v {sh}); then
      export NAMI_LOGIN_SHELL={sh}
      if shopt -q login_shell; then exec "$__nami_sh" -l; else exec "$__nami_sh"; fi
    fi
    unset __nami_sh
    ;;
esac
"#
    )
}

/// A home.env value for sh: as written, except that `$VAR` / `${VAR}` in it
/// are other variables (`$HOME/bin`), as in NixOS and home-manager.
pub fn env_sh(v: &str) -> String {
    if v.contains('$') {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`"))
    } else {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
}

/// The same for fish, which writes `${VAR}` as `{$VAR}`.
fn env_fish(v: &str) -> String {
    if v.contains('$') {
        let mut out = String::new();
        let mut rest = v;
        while let Some(i) = rest.find("${") {
            out += &rest[..i];
            match rest[i + 2..].find('}') {
                Some(j) => {
                    out += &format!("{{${}}}", &rest[i + 2..i + 2 + j]);
                    rest = &rest[i + 3 + j..];
                }
                None => {
                    out += &rest[i..];
                    rest = "";
                }
            }
        }
        out += rest;
        format!("\"{}\"", out.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
}

/// `env:` and `aliases:` become small files that shells and the session read
/// (install.sh's profile.d sources `~/.config/nami/env.sh`); `shell:` too.
fn env_files(home: &Home) -> Vec<(String, String)> {
    let shell = shell_files(home);
    if home.env.is_empty() && home.aliases.is_empty() {
        return shell;
    }
    let mut sh = String::from("# generated by nami from your nami file (env, aliases)\n");
    let mut fish = sh.clone();
    for (k, v) in &home.env {
        if crate::secrets::marked(v).is_none() {
            sh += &format!("export {k}={}\n", env_sh(v));
            fish += &format!("set -gx {k} {}\n", env_fish(v));
        }
    }
    if home.env.values().any(|v| crate::secrets::marked(v).is_some()) {
        // Secret values live outside the store, in private files nami writes on activation.
        sh += "[ -r \"$HOME/.config/nami/secrets.env\" ] && . \"$HOME/.config/nami/secrets.env\"\n";
        fish += "test -r \"$HOME/.config/nami/secrets.fish\"; and source \"$HOME/.config/nami/secrets.fish\"\n";
    }
    for (k, v) in &home.aliases {
        sh += &format!("alias {k}={}\n", sh_quote(v));
        fish += &format!("alias {k} {}\n", sh_quote(v));
    }
    let mut files = vec![
        (".config/nami/env.sh".to_string(), sh),
        (".config/fish/conf.d/nami-home.fish".to_string(), fish),
    ];
    if !home.env.is_empty() {
        // Graphical sessions (systemd user environment).
        let mut d = String::from("# generated by nami from your nami file (env)\n");
        for (k, v) in &home.env {
            if crate::secrets::marked(v).is_none() {
                d += &format!("{k}={v}\n");
            }
        }
        files.push((".config/environment.d/60-nami-home.conf".to_string(), d));
    }
    files.extend(shell);
    files
}

/// What the last activation did with each file, to know what's ours.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Managed {
    #[serde(default)]
    mutable: bool,
    /// Mutable files: the hash of what nami last wrote, to tell whether
    /// you (or an app) changed it since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    written: Option<String>,
    /// Mutable files: the spec's version they were written from, so a later
    /// change to the spec can be merged with the app's changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spec: Option<String>,
    /// Decrypted from a secret: nami's to replace and remove.
    #[serde(default)]
    secret: bool,
}

/// What an app changed in one of its settings files since nami wrote it.
#[derive(Debug, Serialize)]
pub struct Drift {
    /// Relative to ~.
    pub file: String,
    /// Setting by setting; empty when the file is plain text (see `whole`).
    pub changes: Vec<crate::drift::Change>,
    /// The file changed, but it's text nami can't compare setting by setting.
    pub whole: bool,
}

/// Editable (`mutable`) files that no longer say what the spec says.
pub fn drift(home_dir: &Path, link_base: &Path) -> Result<Vec<Drift>> {
    let state_file = home_dir.join(".local/state/nami/home-files.json");
    let managed: BTreeMap<String, Managed> = fs::read(&state_file).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    let mut out = Vec::new();
    for (rel, _) in managed.iter().filter(|(_, m)| m.mutable) {
        let (Ok(spec), Ok(now)) = (fs::read_to_string(link_base.join(rel)), fs::read_to_string(home_dir.join(rel))) else { continue };
        if spec == now {
            continue;
        }
        let parsed = (crate::drift::parse(rel, &spec), crate::drift::parse(rel, &now));
        match parsed {
            (Ok(Some(a)), Ok(Some(b))) => {
                let changes = crate::drift::diff(&a, &b);
                if !changes.is_empty() {
                    out.push(Drift { file: rel.clone(), changes, whole: false });
                }
            }
            _ => out.push(Drift { file: rel.clone(), changes: Vec::new(), whole: true }),
        }
    }
    Ok(out)
}

/// Put the spec's version of an editable file back, throwing away what an
/// app changed in it.
pub fn undo(home_dir: &Path, link_base: &Path, rel: &str) -> Result<()> {
    let state_file = home_dir.join(".local/state/nami/home-files.json");
    let mut managed: BTreeMap<String, Managed> = fs::read(&state_file).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    let m = managed.get_mut(rel).filter(|m| m.mutable).ok_or_else(|| anyhow!("~/{rel} isn't an editable settings file from your spec"))?;
    let target = link_base.join(rel);
    let dest = home_dir.join(rel);
    if dest.symlink_metadata().is_ok() {
        remove(&dest)?;
    }
    copy_writable(&target, &dest)?;
    m.written = hash(&dest);
    write_atomic(&state_file, &serde_json::to_vec_pretty(&managed)?)
}

/// Make ~ match `files`. Most are links to `<link_base>/<file>` (inside the
/// active profile, so switching generations updates them all at once); the
/// `mutable` ones are copies you and your apps may edit. Unlinks/removes what
/// the previous activation managed but this one doesn't.
pub fn activate(home_dir: &Path, link_base: &Path, state: Option<&HomeState>) -> Result<()> {
    activate_with(home_dir, link_base, state, &crate::secrets::Keys::mine())
}

/// [`activate`], opening secrets with `keys`.
fn activate_with(home_dir: &Path, link_base: &Path, state: Option<&HomeState>, keys: &crate::secrets::Keys) -> Result<()> {
    let empty = Vec::new();
    let files = state.map(|h| &h.files).unwrap_or(&empty);
    let mutable = state.map(|h| &h.mutable).unwrap_or(&empty);
    let no_modes = BTreeMap::new();
    let modes = state.map(|h| &h.modes).unwrap_or(&no_modes);
    let secret_files: Vec<&SecretFile> = state.map(|h| h.secret_files.iter().collect()).unwrap_or_default();
    let state_file = home_dir.join(".local/state/nami/home-files.json");
    // (tray's list, in a home nami hasn't moved yet.)
    let read = fs::read(&state_file).or_else(|_| fs::read(home_dir.join(".local/state/tray/home-files.json")));
    let previous: BTreeMap<String, Managed> = match read {
        Ok(d) => serde_json::from_slice(&d)
            .or_else(|_| serde_json::from_slice::<Vec<String>>(&d).map(|v| v.into_iter().map(|f| (f, Managed { mutable: false, written: None, spec: None, secret: false })).collect()))
            .unwrap_or_default(),
        Err(_) => BTreeMap::new(),
    };
    // Each by the place it is in ~ (tray's ~/.config/tray/... is
    // ~/.config/nami/... now, the old name a link to the new): one file
    // under two names is one file, never "the old one" to remove.
    let previous: BTreeMap<String, (String, Managed)> = previous.into_iter().map(|(rel, m)| (place(home_dir, &rel), (rel, m))).collect();
    let managed: std::collections::BTreeSet<String> = files.iter().chain(secret_files.iter().map(|s| &s.rel)).map(|r| place(home_dir, r)).collect();
    let was_at = |rel: &str| previous.get(&place(home_dir, rel)).map(|(_, m)| m);
    let mut now: BTreeMap<String, Managed> = BTreeMap::new();

    for (rel, (named, was)) in &previous {
        if managed.contains(rel) {
            continue;
        }
        let dest = home_dir.join(rel);
        if was.secret {
            if dest.symlink_metadata().is_ok() {
                remove(&dest)?;
            }
            restore_backup(&dest, rel)?;
        } else if was.mutable {
            if hash(&dest).is_some() && hash(&dest) == was.written {
                remove(&dest)?;
                restore_backup(&dest, rel)?;
            } else if dest.symlink_metadata().is_ok() {
                crate::say!("kept ~/{rel}: you changed it, so it's yours now (no longer managed)");
            }
        } else if fs::read_link(&dest).ok().is_some_and(|l| crate::oldnames::same_place(&l, &link_base.join(rel)) || crate::oldnames::same_place(&l, &link_base.join(named))) {
            fs::remove_file(&dest)?;
            restore_backup(&dest, rel)?;
        }
    }

    for rel in files {
        let dest = home_dir.join(rel);
        let target = link_base.join(rel);
        let was = was_at(rel);
        let link = fs::read_link(&dest).ok();
        let ours_link = link.as_ref().is_some_and(|l| crate::oldnames::same_place(l, &target));
        let current = if ours_link { None } else { hash(&dest) };
        let untouched = was.is_some_and(|w| w.mutable && current.is_some() && current == w.written);

        if mutable.contains(rel) {
            let wanted = hash(&target).ok_or_else(|| anyhow!("{} is missing", target.display()))?;
            let edited = was.is_some_and(|w| w.mutable) && !untouched && current.is_some();
            let yaml_changed = was.and_then(|w| w.written.as_ref()) != Some(&wanted);
            let same_settings = || -> bool {
                let parse = |p: &Path| fs::read_to_string(p).ok().and_then(|t| crate::drift::parse(rel, &t).ok().flatten());
                matches!((parse(&dest), parse(&target)), (Some(a), Some(b)) if crate::drift::same(&a, &b))
            };
            if current.as_ref() == Some(&wanted) {
                // Already what we want.
            } else if !ours_link && current.is_some() && was.is_none() && same_settings() {
                // A file you already had, saying what the spec says (just
                // laid out differently): it's managed now, as it is.
                now.insert(rel.clone(), Managed { mutable: true, written: current.clone(), spec: fs::read_to_string(&target).ok(), secret: false });
                continue;
            } else if edited && !yaml_changed {
                // You or an app changed it, and nami file didn't: leave it.
                now.insert(rel.clone(), was.unwrap().clone());
                continue;
            } else if let Some(merged) = edited.then(|| merge_edits(rel, &dest, &target, was.and_then(|w| w.spec.as_deref()))).flatten() {
                // Both changed: setting by setting, each side keeps its changes.
                let (text, conflicts) = merged;
                if !conflicts.is_empty() {
                    let backup = free_backup(&dest);
                    fs::copy(&dest, &backup)?;
                    for c in &conflicts {
                        crate::say!("~/{rel}: {} changed in both the app and your spec; your spec wins (the app's version is in ~/{})", c.join("."), backup.strip_prefix(home_dir).unwrap_or(&backup).display());
                    }
                }
                write_writable(&dest, &text)?;
                if let Some(mode) = modes.get(rel) {
                    fs::set_permissions(&dest, fs::Permissions::from_mode(*mode))?;
                }
                crate::say!("merged ~/{rel}: your spec's changes, and the app's");
                now.insert(rel.clone(), Managed { mutable: true, written: hash(&dest), spec: fs::read_to_string(&target).ok(), secret: false });
                continue;
            } else {
                if ours_link || untouched {
                    remove(&dest)?;
                } else if dest.symlink_metadata().is_ok() {
                    let backup = free_backup(&dest);
                    fs::rename(&dest, &backup)?;
                    crate::say!("moved your ~/{rel} to ~/{}", backup.strip_prefix(home_dir).unwrap_or(&backup).display());
                }
                fs::create_dir_all(dest.parent().unwrap())?;
                copy_writable(&target, &dest)?;
            }
            if let Some(mode) = modes.get(rel) {
                fs::set_permissions(&dest, fs::Permissions::from_mode(*mode))?;
            }
            now.insert(rel.clone(), Managed { mutable: true, written: Some(wanted), spec: fs::read_to_string(&target).ok(), secret: false });
            continue;
        }

        now.insert(rel.clone(), Managed { mutable: false, written: None, spec: None, secret: false });
        if ours_link {
            if link.as_deref() == Some(target.as_path()) {
                continue;
            }
            // tray's link (/nix/var/tray/.../.config/tray/env.sh): ours, but
            // to where the file was; in this generation it's by nami's name.
            fs::remove_file(&dest)?;
        }
        if untouched {
            remove(&dest)?; // was a mutable copy nobody changed; becomes a link
        } else if dest.symlink_metadata().is_ok() {
            let backup = free_backup(&dest);
            fs::rename(&dest, &backup)?;
            crate::say!("moved your ~/{rel} to ~/{}", backup.strip_prefix(home_dir).unwrap_or(&backup).display());
        }
        fs::create_dir_all(dest.parent().unwrap())?;
        symlink(&target, &dest)?;
    }
    // Secrets: decrypted straight into place, private to you.
    for sf in &secret_files {
        let dest = home_dir.join(&sf.rel);
        let ours = was_at(&sf.rel).is_some_and(|w| w.secret);
        if !ours && dest.symlink_metadata().is_ok() {
            let backup = free_backup(&dest);
            fs::rename(&dest, &backup)?;
            crate::say!("moved your ~/{} to ~/{}", sf.rel, backup.strip_prefix(home_dir).unwrap_or(&backup).display());
        }
        let data = sf.render(keys).map_err(|e| anyhow!("~/{}: can't open its secret: {e:#}; {}", sf.rel, crate::secrets::access_hint()))?;
        crate::secrets::install_file(&dest, &data, sf.mode)?;
        now.insert(sf.rel.clone(), Managed { mutable: false, written: None, spec: None, secret: true });
    }
    write_secret_env(home_dir, keys, state.map(|h| h.secret_env.as_slice()).unwrap_or(&[]))?;

    fs::create_dir_all(state_file.parent().unwrap())?;
    crate::store::write_atomic(&state_file, &serde_json::to_vec_pretty(&now)?)?;
    hook_shell_rcs(home_dir, now.contains_key(".config/nami/env.sh"), now.contains_key(LOGIN_FILE), |sh| now.contains_key(&format!(".config/nami/shell.{sh}")))?;
    let none = crate::settings::Settings::new();
    crate::settings::activate(home_dir, state.map(|h| &h.settings).unwrap_or(&none))
}

/// Where the file `rel` (relative to ~, by its name now or tray's) is in
/// `home_dir`, relative to it: through a folder that's a link within ~
/// (`~/.config/tray -> nami`) to the folder itself; tray's `.config/tray/...`
/// as nami's when that folder isn't there.
fn place(home_dir: &Path, rel: &str) -> String {
    let real = |rel: &str| -> Option<String> {
        let p = Path::new(rel);
        let dir = home_dir.join(p.parent()?).canonicalize().ok()?;
        let at = dir.strip_prefix(home_dir.canonicalize().ok()?).ok()?.join(p.file_name()?);
        Some(at.to_string_lossy().into_owned())
    };
    let nami = crate::oldnames::nami_place(rel);
    real(rel).or_else(|| real(&nami)).unwrap_or(nami)
}

/// home.env.`var`'s secret, opened: one line.
fn env_secret(keys: &crate::secrets::Keys, var: &str, sealed: &str) -> Result<String> {
    let value = keys.reveal(sealed).map_err(|e| anyhow!("env.{var}: can't open its secret: {e:#}; {}", crate::secrets::access_hint()))?;
    let value = value.strip_suffix('\n').unwrap_or(&value).to_string();
    ensure!(!value.contains('\n'), "env.{var}: its secret has several lines; an environment variable is one");
    Ok(value)
}

/// Secret env vars, for shells (`secrets.env`, `secrets.fish`) and the
/// desktop session (environment.d), all mode 0600; removed when unused.
fn write_secret_env(home_dir: &Path, keys: &crate::secrets::Keys, vars: &[(String, String)]) -> Result<()> {
    let files = [
        home_dir.join(".config/nami/secrets.env"),
        home_dir.join(".config/nami/secrets.fish"),
        home_dir.join(".config/environment.d/61-nami-secrets.conf"),
    ];
    if vars.is_empty() {
        for f in &files {
            let _ = fs::remove_file(f);
        }
        return Ok(());
    }
    let q =|s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let (mut sh, mut fish, mut envd) = (String::from("# written by nami from encrypted secrets; private\n"), String::new(), String::new());
    fish.push_str(&sh);
    envd.push_str(&sh);
    for (var, sealed) in vars {
        let value = env_secret(keys, var, sealed)?;
        let value = value.as_str();
        sh += &format!("export {var}={}\n", q(value));
        fish += &format!("set -gx {var} {}\n", q(value));
        envd += &format!("{var}={value}\n");
    }
    for (f, text) in files.iter().zip([sh, fish, envd]) {
        crate::secrets::install_file(f, text.as_bytes(), 0o600)?;
    }
    Ok(())
}

/// NAR hash of a file or directory; `None` if it doesn't exist.
/// Three-way merge of an edited settings file with a changed spec: None
/// when the file isn't settings nami can read (then the spec replaces it).
fn merge_edits(rel: &str, dest: &Path, target: &Path, old_spec: Option<&str>) -> Option<(String, Vec<Vec<String>>)> {
    let parse = |t: &str| crate::drift::parse(rel, t).ok().flatten();
    let base = parse(old_spec?)?;
    let ours = parse(&fs::read_to_string(target).ok()?)?;
    let theirs = parse(&fs::read_to_string(dest).ok()?)?;
    let (merged, conflicts) = crate::drift::merge(&base, &ours, &theirs);
    Some((render(rel, &merged).ok()?, conflicts))
}

fn write_writable(dest: &Path, text: &str) -> Result<()> {
    if dest.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        fs::remove_file(dest)?;
    }
    fs::write(dest, text)?;
    fs::set_permissions(dest, fs::Permissions::from_mode(0o644))?;
    Ok(())
}

fn hash(path: &Path) -> Option<String> {
    path.symlink_metadata().ok()?;
    crate::nar::hash_path(path).ok().map(|(h, _)| h)
}

fn remove(path: &Path) -> Result<()> {
    if path.symlink_metadata()?.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn restore_backup(dest: &Path, rel: &str) -> Result<()> {
    let mut backup = backup_of(dest);
    // (One tray kept: `<file>.tray-backup`.)
    if backup.symlink_metadata().is_err() {
        let mut s = dest.as_os_str().to_owned();
        s.push(".tray-backup");
        backup = PathBuf::from(s);
    }
    if backup.symlink_metadata().is_ok() && dest.symlink_metadata().is_err() {
        fs::rename(&backup, dest)?;
        crate::say!("restored ~/{rel} from its backup");
    }
    Ok(())
}

/// `<file>.nami-backup`, or `<file>.nami-backup-<n>` if that's taken.
fn free_backup(dest: &Path) -> PathBuf {
    let first = backup_of(dest);
    if first.symlink_metadata().is_err() {
        return first;
    }
    (1..).map(|n| PathBuf::from(format!("{}-{n}", first.display()))).find(|p| p.symlink_metadata().is_err()).unwrap()
}

/// Copy out of the (read-only) store, making the copy yours to edit.
fn copy_writable(src: &Path, dest: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    copy_tree(src, dest)?;
    fn fix(p: &Path) -> Result<()> {
        let m = p.symlink_metadata()?;
        if m.file_type().is_symlink() {
            return Ok(());
        }
        let exec = m.is_dir() || m.permissions().mode() & 0o111 != 0;
        fs::set_permissions(p, fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 }))?;
        if m.is_dir() {
            for e in fs::read_dir(p)? {
                fix(&e?.path())?;
            }
        }
        Ok(())
    }
    fix(dest)
}

const LOGIN_LINE: &str = "[ -r \"$HOME/.config/nami/login.bash\" ] && . \"$HOME/.config/nami/login.bash\"  # added by nami (home.login-shell)";
const RC_LINE: &str = "[ -r \"$HOME/.config/nami/env.sh\" ] && . \"$HOME/.config/nami/env.sh\"  # added by nami (env, aliases)";

/// The line that runs `home.shell` for `sh` (bash or zsh), last in its rc.
/// One of nami's lines in a shell's file as tray wrote it.
fn tray_line(nami: &str) -> String {
    nami.replace("/.config/nami/", "/.config/tray/").replace("# added by nami", "# added by tray")
}

fn shell_line(sh: &str) -> String {
    format!("[ -r \"$HOME/.config/nami/shell.{sh}\" ] && . \"$HOME/.config/nami/shell.{sh}\"  # added by nami (home.shell)")
}

/// Your own ~/.bashrc and ~/.zshrc run after the system-wide files (and
/// distros' defaults define aliases like `ll`), so nami's settings have to be
/// sourced at their end to win. Marked lines, removed again when unused:
/// env.sh's in an rc that exists, and in ~/.bashrc made for it if it's missing
/// (terminals start bash without a login, so the profile's sourcing of it is
/// never read there: a fresh account's aliases would be nowhere); home.shell's,
/// which is only for that rc, creates it if it's missing (a fresh account has
/// no ~/.zshrc). An rc that's a link belongs to someone else (home.files, a
/// dotfiles manager): left alone. One left with nothing but ours goes with it.
fn hook_shell_rcs(home_dir: &Path, env: bool, login: bool, shell: impl Fn(&str) -> bool) -> Result<()> {
    for (rc, sh) in [(".bashrc", "bash"), (".zshrc", "zsh")] {
        let path = home_dir.join(rc);
        let login = login && sh == "bash";
        let want_shell = shell(sh) || login;
        let meta = fs::symlink_metadata(&path);
        if meta.as_ref().is_ok_and(|m| !m.is_file()) {
            if want_shell {
                let hm = fs::read_link(&path).is_ok_and(|t| t.to_string_lossy().contains("home-manager"));
                if hm {
                    crate::say!("~/{rc} is still home-manager's, so home.shell.{sh} isn't hooked into it: remove home-manager (and switch), then `nami apply` again");
                } else {
                    let what = if login { format!("home.login-shell ({LOGIN_LINE})") } else { format!("home.shell.{sh}") };
                    crate::say!("~/{rc} isn't a plain file, so {what} isn't hooked into it; add this line at its end:\n  {}", if login { LOGIN_LINE.to_string() } else { shell_line(sh) });
                }
            }
            continue;
        }
        if meta.is_err() && !want_shell && !(env && sh == "bash") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        let line = shell_line(sh);
        let has_shell = shell(sh);
        // Lines already there stay where they are; missing ones go last,
        // env.sh's before home.shell's, and the switch to another shell last.
        let want = |l: &str| (l == RC_LINE && env) || (l == line && has_shell) || (l == LOGIN_LINE && login);
        let ours = |l: &str| l == RC_LINE || l == line || l == LOGIN_LINE;
        // tray's lines (`~/.config/tray/env.sh ... # added by tray`) become nami's, in their place.
        let trays = [RC_LINE, line.as_str(), LOGIN_LINE].map(|n| (tray_line(n), n));
        let now = |l: &'_ str| -> String { trays.iter().find(|(t, _)| t == l).map_or_else(|| l.to_string(), |(_, n)| n.to_string()) };
        let renamed: Vec<String> = text.lines().map(now).collect();
        let mut lines: Vec<&str> = renamed.iter().map(String::as_str).filter(|l| !ours(l) || want(l)).collect();
        if env && !lines.contains(&RC_LINE) {
            lines.push(RC_LINE);
        }
        if has_shell && !lines.contains(&line.as_str()) {
            lines.push(&line);
        }
        if login && !lines.contains(&LOGIN_LINE) {
            lines.push(LOGIN_LINE);
        }
        if lines.iter().copied().eq(text.lines()) && meta.is_ok() {
            continue; // nothing of ours added or removed
        }
        if lines.is_empty() {
            if meta.is_ok() {
                fs::remove_file(&path)?;
            }
            continue;
        }
        fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>())?;
    }
    Ok(())
}

fn backup_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".nami-backup");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    /// A `source:` in your home is linked to; from elsewhere, or a copy you
    /// may change (mutable), it isn't.
    #[test]
    fn sources_in_your_home_are_live() {
        let d = std::env::temp_dir().join(format!("nami-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (home, other) = (d.join("home"), d.join("other"));
        for dir in [home.join(".config/nami/files/nvim"), home.join(".cache/nami/layers/x"), other.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let v = |s: &str| serde_yaml::from_str::<serde_yaml::Value>(s).unwrap();
        let repo = home.join(".config/nami");
        assert_eq!(super::live_source(&v("source: ./files/nvim"), &repo, &home), Some(repo.join("files/nvim").canonicalize().unwrap()));
        assert_eq!(super::live_source(&v("source: ./files/nvim"), &other, &home), None);
        assert_eq!(super::live_source(&v("source: ."), &home.join(".cache/nami/layers/x"), &home), None);
        assert_eq!(super::live_source(&v("{ source: ./files/nvim, mutable: true }"), &repo, &home), None);
        assert_eq!(super::live_source(&v("source: ./missing"), &repo, &home), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    use super::*;

    fn y(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn gitconfig_with_subsections() {
        let v = y("user: { name: Ada, email: j@example.org }\nremote:\n  origin: { url: 'git@x:y.git', fetch: ['+a', '+b'] }\ncore: { autocrlf: false }");
        assert_eq!(render(".gitconfig", &v).unwrap(), "[user]\n\tname = Ada\n\temail = j@example.org\n[remote \"origin\"]\n\turl = git@x:y.git\n\tfetch = +a\n\tfetch = +b\n[core]\n\tautocrlf = false\n");
    }

    #[test]
    fn toml_json_yaml_keep_order() {
        let v = y("zeta: 1\nalpha: { size: 12 }");
        assert_eq!(render("a.toml", &v).unwrap(), "zeta = 1\n\n[alpha]\nsize = 12\n");
        assert_eq!(render("a.json", &v).unwrap(), "{\n  \"zeta\": 1,\n  \"alpha\": {\n    \"size\": 12\n  }\n}\n");
        assert_eq!(render("a.yml", &v).unwrap(), "zeta: 1\nalpha:\n  size: 12\n");
    }

    #[test]
    fn mutable_wrapper() {
        let wrapped = y("mutable: true\ncontent: { a: 1 }");
        let (v, m, _) = unwrap_options(&wrapped).unwrap();
        assert!(m);
        assert_eq!(v, &y("a: 1"));
        // A settings file that merely has a `content` key among others stays settings.
        let s = y("content: x\nother: 2");
        assert_eq!(unwrap_options(&s).unwrap(), (&s, false, None));
        assert!(unwrap_options(&y("mutable: yes-please\ncontent: x")).is_err());
        // `mutable:` alone is a setting: options are only with content: or source:.
        let alone = y("mutable: true\ntheme: dark");
        assert_eq!(unwrap_options(&alone).unwrap(), (&alone, false, None));
    }

    #[test]
    fn a_home_file_takes_the_same_options() {
        // A mode: is the file's, not a setting in it.
        let v = y("content: x\nmode: \"0400\"");
        assert_eq!(unwrap_options(&v).unwrap(), (&Value::from("x"), false, Some(0o400)));
        let v = y("content: x\nmode: \"0600\"\nmutable: true");
        assert_eq!(unwrap_options(&v).unwrap(), (&Value::from("x"), true, Some(0o600)));
        // The computer's options aren't a home file's.
        let e = unwrap_options(&y("content: x\nowner: ada")).unwrap_err().to_string();
        assert!(e.contains("owner: is for the computer's files in /etc"), "{e}");
        // A file with its own mode, no secret in it: written in place, private.
        let sf = secret_file(".netrc", &y("content: 'machine x password y'\nmode: \"0400\"")).unwrap();
        assert_eq!(sf.mode, 0o400);
    }

    #[test]
    fn unknown_format_is_an_error() {
        assert!(render(".bashrc", &y("a: 1")).is_err());
    }

    #[test]
    fn paths_stay_in_home() {
        assert!(check_rel("../etc/passwd").is_err());
        assert!(check_rel("/etc/passwd").is_err());
        assert_eq!(check_rel(".config//foo/bar.toml").unwrap(), ".config/foo/bar.toml");
    }

    #[test]
    fn env_values_may_use_other_variables() {
        assert_eq!(env_sh("nvim"), "'nvim'");
        assert_eq!(env_sh("$HOME/bin:${XDG_DATA_HOME}/x \"q\""), "\"$HOME/bin:${XDG_DATA_HOME}/x \\\"q\\\"\"");
        assert_eq!(env_fish("${XDG_CONFIG_HOME}/aspell.conf"), "\"{$XDG_CONFIG_HOME}/aspell.conf\"");
        assert_eq!(env_fish("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_lines_per_shell() {
        let home: Home = serde_yaml::from_str("shell:\n  zsh: eval \"$(starship init zsh)\"\n  fish: |\n    starship init fish | source\n").unwrap();
        let files: BTreeMap<_, _> = env_files(&home).into_iter().collect();
        let zsh = &files[".config/nami/shell.zsh"];
        assert!(zsh.contains("nami_share() {"));
        assert!(zsh.ends_with("eval \"$(starship init zsh)\"\n"));
        assert_eq!(files[".config/fish/conf.d/nami-shell.fish"].lines().skip(1).collect::<Vec<_>>(), ["if status is-interactive", "starship init fish | source", "end"]);
        assert!(!files.contains_key(".config/nami/env.sh"), "no env or aliases: no env.sh");
    }

    #[test]
    fn aliases_win_over_what_shell_lines_load() {
        // oh-my-zsh defines its own `ll`: yours come again after it.
        let home: Home = serde_yaml::from_str("aliases: { ll: eza -l }\nshell:\n  zsh: source \"$ZSH/oh-my-zsh.sh\"\n  fish: set -g x 1\n").unwrap();
        let files: BTreeMap<_, _> = env_files(&home).into_iter().collect();
        assert!(files[".config/nami/env.sh"].contains("alias ll='eza -l'\n"));
        assert!(files[".config/nami/shell.zsh"].ends_with("source \"$ZSH/oh-my-zsh.sh\"\n# your aliases, over what the lines above defined\nalias ll='eza -l'\n"));
        assert!(files[".config/fish/conf.d/nami-shell.fish"].ends_with("set -g x 1\n# your aliases, over what the lines above defined\nalias ll 'eza -l'\nend\n"));
    }

    #[test]
    fn nami_share_finds_a_package_file() {
        let dir = std::env::temp_dir().join(format!("nami-share-test-{}", std::process::id()));
        let share = dir.join(".local/state/nami/profile/share/zsh-x");
        fs::create_dir_all(&share).unwrap();
        fs::write(share.join("x.zsh"), "").unwrap();
        for sh in ["sh", "bash", "zsh"] {
            if std::process::Command::new(sh).arg("-c").arg("true").output().is_err() {
                continue;
            }
            let out = std::process::Command::new(sh)
                .arg("-c")
                .arg(format!("{}nami_share zsh/new/x.zsh zsh-x/x.zsh; nami_share nope 2>/dev/null || echo missing", nami_share_sh()))
                .env("HOME", &dir)
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{}\nmissing\n", share.join("x.zsh").display()), "{sh}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn secrets_open_into_private_files() {
        let tmp = std::env::temp_dir().join(format!("nami-home-secrets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let id = age::x25519::Identity::generate();
        let public = id.to_public().to_string();
        let keys = crate::secrets::Keys::of(id);
        let token = crate::secrets::marked_for("ghp_123", &public);
        let files: Mapping = serde_yaml::from_str(&format!(".config/rclone/rclone.conf:\n  s3: {{ type: s3, secret_access_key: '{}' }}\n.netrc: {{ content: '{}', mode: '0400' }}\n", crate::secrets::marked_for("AKIA", &public), crate::secrets::marked_for("machine x password y\n", &public))).unwrap();
        let secret_files: Vec<SecretFile> = files.iter().map(|(k, v)| secret_file(k.as_str().unwrap(), v).unwrap()).collect();
        let state = HomeState { path: tmp.display().to_string(), files: Vec::new(), mutable: Vec::new(), modes: Default::default(), secret_files, secret_env: vec![("GITHUB_TOKEN".into(), token)], settings: Default::default() };
        let home = tmp.join("home");
        fs::create_dir_all(&home).unwrap();
        activate_with(&home, &tmp, Some(&state), &keys).unwrap();
        let rclone = home.join(".config/rclone/rclone.conf");
        assert_eq!(fs::read_to_string(&rclone).unwrap(), "[s3]\ntype = s3\nsecret_access_key = AKIA\n");
        assert_eq!(fs::metadata(&rclone).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(home.join(".netrc")).unwrap().permissions().mode() & 0o777, 0o400);
        let env = home.join(".config/nami/secrets.env");
        assert!(fs::read_to_string(&env).unwrap().contains("export GITHUB_TOKEN='ghp_123'\n"));
        assert_eq!(fs::metadata(&env).unwrap().permissions().mode() & 0o777, 0o600);
        // What's kept in the generation stays sealed.
        assert!(!serde_json::to_string(&state).unwrap().contains("ghp_123"));
        // Someone without the key: says which, and how to get access.
        let other = crate::secrets::Keys::of(age::x25519::Identity::generate());
        let e = activate_with(&home, &tmp, Some(&state), &other).unwrap_err().to_string();
        assert!(e.contains("can't open its secret") && e.contains("nami secret allow"), "{e}");
        // Gone with the file.
        activate_with(&home, &tmp, None, &keys).unwrap();
        assert!(!rclone.exists() && !env.exists());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn rc_lines_added_kept_and_removed() {
        let dir = std::env::temp_dir().join(format!("nami-rc-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".bashrc"), "alias ll='ls -l'\n").unwrap();
        let rc = |f: &str| fs::read_to_string(dir.join(f)).unwrap_or_default();
        // env for both, home.shell for zsh only: ~/.zshrc didn't exist, so it's made.
        hook_shell_rcs(&dir, true, false, |sh| sh == "zsh").unwrap();
        assert_eq!(rc(".bashrc"), format!("alias ll='ls -l'\n{RC_LINE}\n"));
        assert_eq!(rc(".zshrc"), format!("{RC_LINE}\n{}\n", shell_line("zsh")));
        // Your lines after ours stay after them; nothing changes twice.
        fs::write(dir.join(".zshrc"), rc(".zshrc") + "export X=1\n").unwrap();
        hook_shell_rcs(&dir, true, false, |sh| sh == "zsh").unwrap();
        assert!(rc(".zshrc").ends_with("export X=1\n"));
        // Unused: our lines go, yours stay.
        hook_shell_rcs(&dir, false, false, |_| false).unwrap();
        assert_eq!(rc(".bashrc"), "alias ll='ls -l'\n");
        assert_eq!(rc(".zshrc"), "export X=1\n");
        // home.login-shell: ~/.bashrc only, after the rest.
        hook_shell_rcs(&dir, true, true, |_| false).unwrap();
        assert_eq!(rc(".bashrc"), format!("alias ll='ls -l'\n{RC_LINE}\n{LOGIN_LINE}\n"));
        assert!(!rc(".zshrc").contains("login"));
        hook_shell_rcs(&dir, true, false, |_| false).unwrap();
        assert_eq!(rc(".bashrc"), format!("alias ll='ls -l'\n{RC_LINE}\n"));
        // No ~/.bashrc (a fresh account): made for env.sh, as terminals' bash reads only it;
        // gone again with it. ~/.zshrc isn't made for env.sh alone.
        fs::remove_file(dir.join(".bashrc")).unwrap();
        fs::remove_file(dir.join(".zshrc")).unwrap();
        hook_shell_rcs(&dir, true, false, |_| false).unwrap();
        assert_eq!(rc(".bashrc"), format!("{RC_LINE}\n"));
        // tray's line becomes nami's where it was, never both.
        fs::write(dir.join(".bashrc"), format!("a\n{}\nb\n", tray_line(RC_LINE))).unwrap();
        hook_shell_rcs(&dir, true, false, |_| false).unwrap();
        assert_eq!(rc(".bashrc"), format!("a\n{RC_LINE}\nb\n"));
        fs::write(dir.join(".bashrc"), format!("{RC_LINE}\n")).unwrap();
        assert!(!dir.join(".zshrc").exists());
        hook_shell_rcs(&dir, false, false, |_| false).unwrap();
        assert!(!dir.join(".bashrc").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Links an older nami made name nami's folder by its old place, /nix/var/nami: moved to
    /// /nami since, they're nami's still, made links to /nami's, nothing kept as someone's.
    #[test]
    fn links_by_the_folders_old_place_are_namis() {
        let t = std::env::temp_dir().join(format!("nami-home-top-{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        let profile = t.join("nami/profiles/per-user/ada");
        let at = profile.join("default-2/.home/.config/app/conf");
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, "x = 1\n").unwrap();
        symlink("default-2", profile.join("default")).unwrap();
        fs::create_dir_all(t.join("nix/var")).unwrap();
        symlink("../../nami", t.join("nix/var/nami")).unwrap();
        let link_base = profile.join("default/.home");
        let old_base = t.join("nix/var/nami/profiles/per-user/ada/default/.home");
        let home = t.join("home");
        for d in [".config/app", ".local/state/nami"] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        symlink(old_base.join(".config/app/conf"), home.join(".config/app/conf")).unwrap();
        let listed: BTreeMap<&str, Managed> = [(".config/app/conf", Managed { mutable: false, written: None, spec: None, secret: false })].into_iter().collect();
        fs::write(home.join(".local/state/nami/home-files.json"), serde_json::to_vec(&listed).unwrap()).unwrap();
        let state = HomeState { path: String::new(), files: vec![".config/app/conf".into()], mutable: Vec::new(), modes: Default::default(), secret_files: Vec::new(), secret_env: Vec::new(), settings: Default::default() };
        let keys = crate::secrets::Keys::of(age::x25519::Identity::generate());
        activate_with(&home, &link_base, Some(&state), &keys).unwrap();
        assert_eq!(fs::read_link(home.join(".config/app/conf")).ok(), Some(link_base.join(".config/app/conf")));
        let names: Vec<_> = fs::read_dir(home.join(".config/app")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, ["conf"], "no backup: it was nami's");
        let _ = fs::remove_dir_all(&t);
    }

    /// A home tray set up, moved by the rename (`~/.config/tray -> nami`,
    /// tray's links still pointing at tray's names in /nix/var/tray), its
    /// last list naming tray's places, nami's, or both: an apply ends with
    /// every link at this generation's file, and none of tray's left.
    #[test]
    fn trays_links_and_list_become_namis() {
        let t = std::env::temp_dir().join(format!("nami-home-renamed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        // The profile: /nix/var/nami (tray's /nix/var/tray a link to it), `default` this generation.
        let profile = t.join("nix/var/nami/profiles/per-user/ada");
        let link_base = profile.join("default/.home");
        let spec: Home = serde_yaml::from_str("env: { EDITOR: nvim }\naliases: { ll: ls -l }\nshell: { bash: 'set -o vi', zsh: 'bindkey -v', fish: 'fish_vi_key_bindings' }\n").unwrap();
        let mut files: Vec<String> = Vec::new();
        for (rel, text) in env_files(&spec) {
            let at = profile.join("default-2/.home").join(&rel);
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            fs::write(at, text).unwrap();
            files.push(rel);
        }
        files.sort();
        symlink("default-2", profile.join("default")).unwrap();
        symlink("nami", t.join("nix/var/tray")).unwrap();
        let state = HomeState { path: String::new(), files: files.clone(), mutable: Vec::new(), modes: Default::default(), secret_files: Vec::new(), secret_env: Vec::new(), settings: Default::default() };
        let keys = crate::secrets::Keys::of(age::x25519::Identity::generate());
        let old_base = t.join("nix/var/tray/profiles/per-user/ada/default/.home");
        let plain = || Managed { mutable: false, written: None, spec: None, secret: false };
        // What tray made (by tray's names), at the places the rename moved them to.
        let trays = [
            (".config/nami/env.sh", ".config/tray/env.sh"),
            (".config/nami/shell.bash", ".config/tray/shell.bash"),
            (".config/nami/shell.zsh", ".config/tray/shell.zsh"),
            (".config/fish/conf.d/tray-home.fish", ".config/fish/conf.d/tray-home.fish"),
            (".config/fish/conf.d/tray-shell.fish", ".config/fish/conf.d/tray-shell.fish"),
            (".config/environment.d/60-tray-home.conf", ".config/environment.d/60-tray-home.conf"),
        ];
        let tray_list: Vec<&str> = trays.iter().map(|(_, old)| *old).collect();
        let nami_list: Vec<&str> = trays.iter().map(|(at, _)| *at).collect();
        let both: Vec<&str> = tray_list.iter().chain(nami_list.iter()).copied().collect();
        let home = t.join("home");
        for list in [tray_list, nami_list, both] {
            let _ = fs::remove_dir_all(&home);
            for d in [".config/nami", ".config/fish/conf.d", ".config/environment.d", ".local/state/nami"] {
                fs::create_dir_all(home.join(d)).unwrap();
            }
            symlink("nami", home.join(".config/tray")).unwrap();
            symlink("nami", home.join(".local/state/tray")).unwrap();
            for (at, old) in trays {
                symlink(old_base.join(old), home.join(at)).unwrap();
            }
            fs::write(home.join(".bashrc"), format!("alias x=y\n{}\n{}\n", tray_line(RC_LINE), tray_line(&shell_line("bash")))).unwrap();
            fs::write(home.join(".zshrc"), format!("{}\n{}\n", tray_line(RC_LINE), tray_line(&shell_line("zsh")))).unwrap();
            let listed: BTreeMap<&str, Managed> = list.iter().map(|f| (*f, plain())).collect();
            // (Written through tray's name for the folder, as tray did.)
            fs::write(home.join(".local/state/tray/home-files.json"), serde_json::to_vec(&listed).unwrap()).unwrap();
            for round in 0..2 {
                activate_with(&home, &link_base, Some(&state), &keys).unwrap();
                for rel in &files {
                    let dest = home.join(rel);
                    assert_eq!(fs::read_link(&dest).ok(), Some(link_base.join(rel)), "~/{rel} (list {list:?}, round {round})");
                    assert!(dest.is_file(), "~/{rel} leads to a file");
                }
                for gone in [".config/fish/conf.d/tray-home.fish", ".config/fish/conf.d/tray-shell.fish", ".config/environment.d/60-tray-home.conf"] {
                    assert!(home.join(gone).symlink_metadata().is_err(), "~/{gone} is gone (list {list:?})");
                }
                assert!(files.iter().any(|f| f == ".config/fish/conf.d/nami-home.fish") && home.join(".config/fish/conf.d/nami-home.fish").is_file());
                assert!(fs::read_to_string(home.join(".config/tray/env.sh")).unwrap().contains("alias ll='ls -l'"), "and by tray's name for the folder");
                // The shells' hooks: nami's lines, each for a file that's there.
                for (rc, sh) in [(".bashrc", "bash"), (".zshrc", "zsh")] {
                    let text = fs::read_to_string(home.join(rc)).unwrap();
                    assert!(!text.contains("tray") && text.contains(RC_LINE) && text.contains(&shell_line(sh)), "{rc}: {text}");
                    for f in ["env.sh".to_string(), format!("shell.{sh}")] {
                        assert!(home.join(".config/nami").join(f).is_file());
                    }
                }
                let now: BTreeMap<String, Managed> = serde_json::from_slice(&fs::read(home.join(".local/state/nami/home-files.json")).unwrap()).unwrap();
                assert_eq!(now.keys().cloned().collect::<Vec<_>>(), files);
            }
        }
        // A file named through tray's folder (`~/.config/tray/x`), last listed
        // by nami's: one file, kept, not removed as the "old" one.
        let at = profile.join("default-2/.home/.config/tray/x");
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, "x\n").unwrap();
        let mut with_x = state.clone();
        with_x.files.push(".config/tray/x".into());
        fs::write(home.join(".local/state/nami/home-files.json"), serde_json::to_vec(&BTreeMap::from([(".config/nami/x", plain())])).unwrap()).unwrap();
        symlink(link_base.join(".config/tray/x"), home.join(".config/nami/x")).unwrap();
        activate_with(&home, &link_base, Some(&with_x), &keys).unwrap();
        assert_eq!(fs::read_link(home.join(".config/nami/x")).ok(), Some(link_base.join(".config/tray/x")));
        fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "Linux terminals' login shell")]
    fn terminals_switch_to_the_login_shell() {
        let dir = std::env::temp_dir().join(format!("nami-login-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("bin")).unwrap();
        let rc = dir.join("rc");
        fs::write(&rc, login_script("zsh")).unwrap();
        let fake = dir.join("bin/zsh");
        fs::write(&fake, "#!/bin/sh\necho \"zsh $*\"\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        // (Found here: the PATHs below may not have it. namiOS has no /usr/bin.)
        let bash_bin = std::env::split_paths(&std::env::var_os("PATH").unwrap()).map(|d| d.join("bash")).find(|p| p.exists()).unwrap();
        let bash = |path: &str, env: &[(&str, &str)], args: &[&str]| {
            let mut c = std::process::Command::new(&bash_bin);
            c.env_clear().env("PATH", path).env("HOME", &dir).envs(env.iter().copied()).arg("--rcfile").arg(&rc).args(args);
            String::from_utf8(c.output().unwrap().stdout).unwrap().trim().to_string()
        };
        let with = format!("{}:/usr/bin:/bin", dir.join("bin").display());
        assert_eq!(bash(&with, &[], &["-i", "-c", "echo bash"]), "zsh");
        // A bash started from it, a script, or no zsh installed: bash stays.
        assert_eq!(bash(&with, &[("NAMI_LOGIN_SHELL", "zsh")], &["-i", "-c", "echo bash"]), "bash");
        assert_eq!(bash(&with, &[], &["-c", "echo bash"]), "bash");
        assert_eq!(bash("/usr/bin:/bin", &[], &["-i", "-c", "echo bash"]), "bash");
        fs::remove_dir_all(&dir).unwrap();
    }
}
