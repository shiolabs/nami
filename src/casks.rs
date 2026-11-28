//! A Mac's apps from Homebrew's casks: `- cask:rectangle` in a nami file's packages, as a
//! Flathub app is `- flathub:<id>` on Linux. Homebrew itself isn't needed or used: a cask is a
//! recipe (the developer's download, its sha256, which .app goes in Applications), and Homebrew
//! publishes them all as JSON (formulae.brew.sh/api). nami reads that, downloads the app from
//! its developer, checks it, and keeps it once, on the Mac's own volume (`/Library/Application
//! Support/nami/casks`, root's), with a store path of its own that links to it
//! (`cask-rectangle-2.0.5`, the app in its `Applications/`): from there it's like any package
//! with an app. It's in the profile's steps (history, undo, without the network), cloned into
//! nami Apps by mac_apps.rs (one app's space for both; quarantined, as a browser's download is),
//! and `nami gc` takes it once nothing uses it.
//!
//! - **Which casks.** Those that are an app and nothing more: `app` artifacts, programs inside
//!   that app linked into the profile's bin (`binary: $APPDIR/...`, a `command_wrapper`), and
//!   what's only for removing it (`uninstall`, `zap`). A cask that needs Apple's installer
//!   (`pkg`), its own installer, a suite, fonts, plug-ins, steps run as it's installed, other
//!   casks or formulae, another processor or a newer macOS is refused, saying which (`plan`).
//! - **Checked.** The download's sha256 against the cask's. A cask with none (`no_check`: the
//!   developer's link always has the newest, as Chrome's) only if Gatekeeper accepts the app as
//!   notarized by Apple (`spctl --assess`): who made it is then Apple's word, not the link's.
//! - **A plain name.** `- iterm2` is nixpkgs' iterm2 when a Mac's catalog has it ready-built,
//!   else the cask of that name when there's one (`want`), and nami says which it used.
//! - **On Linux** cask entries are left out, said in one line, so one user.nami does for both.
//!
//! The list of casks (each one's name, line, version and whether nami can install it here) is
//! kept in ~/.cache/nami/casks (root's in nami's folder) and fetched again once it's a day old,
//! or with `nami update`; without the network, the one there is used.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::profile::Entry;

/// How a cask is written in packages: `cask:<token>`.
pub const PREFIX: &str = "cask:";
/// An item's `source` (`nami search --json`), as Flathub's is `flathub`.
pub const SOURCE: &str = "cask";
/// Where it's from, as it's said.
pub const FROM: &str = "from Homebrew's casks";
const API: &str = "https://formulae.brew.sh/api";
/// How long the list of casks is kept before it's fetched again.
const LIST_FOR: Duration = Duration::from_secs(24 * 3600);
/// The largest download nami takes, and the largest answer of Homebrew's.
const MAX_DOWNLOAD: u64 = 16 << 30;
const MAX_JSON: u64 = 256 << 20;
/// What a cask's app in nami Apps is marked with (`com.apple.quarantine`): its flags, then when
/// and who. 0x0001 is a download and 0x0080 what browsers add (Chrome's mark is 0081): Gatekeeper
/// checks it and macOS asks before it opens the first time. 0x0100 is what Finder adds as it
/// moves a downloaded app into Applications: without it macOS runs the app from a read-only copy
/// elsewhere (App Translocation, as GitHub's Macs did with 0081), where it can't update itself.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const QUARANTINE_FLAGS: &str = "0181";

pub fn is_cask(p: &str) -> bool {
    p.starts_with(PREFIX)
}

/// The token of a `cask:` entry.
pub fn token(p: &str) -> Option<&str> {
    p.strip_prefix(PREFIX)
}

/// A token as Homebrew has them: lowercase letters, digits, and `-+.@_` (google-chrome@beta).
pub fn valid_token(t: &str) -> bool {
    !t.is_empty() && t.len() <= 128 && t.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-+.@_".contains(c))
}

/// A token and the version asked for, if one was: `rectangle@0.80` is rectangle at 0.80
/// (which a cask can't give: it has its newest alone), unless that's a token itself.
pub fn split_version<'a>(t: &'a str, is_token: impl Fn(&str) -> bool) -> (&'a str, Option<&'a str>) {
    if is_token(t) {
        return (t, None);
    }
    match t.rsplit_once('@') {
        Some((a, v)) if !a.is_empty() && v.starts_with(|c: char| c.is_ascii_digit()) => (a, Some(v)),
        _ => (t, None),
    }
}

/// What's said when a version of a cask is asked for.
pub fn newest_only(token: &str) -> String {
    format!("Homebrew's casks have only the newest version of an app: write cask:{token} (or `nami install cask:{token}`)")
}

/// How `nami check` ends what it says of a `cask:` entry Homebrew hasn't got (as " isn't in the
/// catalog" ends a name's): an apply leaves it out, and goes on.
pub const NOT_A_CASK: &str = " isn't one of Homebrew's casks";

/// What `nami check` says of a `cask:` entry (`p`), if anything: a name no cask could have; on
/// Linux, that it's left out; on a Mac, a version, a name that isn't a cask's, or what makes nami
/// leave it out.
pub fn check_entry(p: &str) -> Option<(crate::config::Severity, String, Option<String>)> {
    use crate::config::Severity;
    let t = token(p)?;
    let (tok, version) = split_version(t, |x| find(x).is_some());
    if !valid_token(tok) {
        return Some((Severity::Error, format!("{p}: a cask is named by its token, as cask:rectangle"), Some("`nami search <word>` finds them, on a Mac".into())));
    }
    if !crate::index::MAC {
        return Some((Severity::Note, format!("{p}: a Mac app from Homebrew's casks, left out on Linux"), None));
    }
    if version.is_some() {
        return Some((Severity::Error, format!("{p}: {}", newest_only(tok)), None));
    }
    match (known(tok), find(tok)) {
        (Some(false), _) => Some((Severity::Error, format!("{p}{NOT_A_CASK}"), Some(format!("`nami search {tok}` looks for it")))),
        (_, Some(s)) => s.refusal.as_ref().map(|r| (Severity::Warning, format!("{p}: it'll be left out: {r}"), None)),
        _ => None,
    }
}

/// What `nami check` says, on a Mac, of a plain name (`p`, `name` without its version) that's
/// Homebrew's cask (nixpkgs has no ready-built Mac build of it): which it'll be, and why.
pub fn check_name(index: &crate::index::Index, p: &str, name: &str, version: Option<&str>) -> Option<(crate::config::Severity, String, Option<String>)> {
    use crate::config::Severity;
    if !crate::index::MAC || !valid_token(name) {
        return None;
    }
    let pkg = index.get(name).ok();
    if pkg.is_some_and(ready_built) {
        return None;
    }
    let s = find(name)?;
    if version.is_some() && index.get(p).is_err() {
        return Some((Severity::Error, format!("{p}: nixpkgs has no ready-built Mac build of it, and {}", newest_only(name)), None));
    }
    let why = if pkg.is_some() { "nixpkgs has no ready-built Mac build of it" } else { "nixpkgs has no package of that name for a Mac" };
    Some(match &s.refusal {
        Some(r) => (Severity::Warning, format!("{p}: it'll be left out: {why}, and of Homebrew's cask, {r}"), None),
        None => (Severity::Note, format!("{p}: {} {FROM} ({why})", s.name), None),
    })
}

/// A profile entry's name: `cask:rectangle-2.0.5`, whatever the file calls it (`rectangle` or
/// `cask:rectangle`). It's what tells a cask's entry from a catalog's.
pub fn entry_name(token: &str, version: &str) -> String {
    format!("{PREFIX}{token}-{version}")
}

pub fn is_cask_entry(e: &Entry) -> bool {
    e.name.starts_with(PREFIX)
}

/// The token of a cask's entry (its attr is the line in the file: `cask:x` or `x`).
pub fn entry_token(e: &Entry) -> Option<&str> {
    is_cask_entry(e).then(|| e.attr.strip_prefix(PREFIX).unwrap_or(&e.attr))
}

/// The version of a cask's entry.
pub fn entry_version(e: &Entry) -> String {
    let token = entry_token(e).unwrap_or_default();
    e.name.strip_prefix(&format!("{PREFIX}{token}-")).map(String::from).unwrap_or_else(|| crate::query::version_of(&e.name))
}

/// The store path's name: `cask-rectangle-2.0.5` (what a store name can't have, as `_`).
pub fn store_name(token: &str, version: &str) -> String {
    let name: String = format!("cask-{token}-{version}").chars().map(|c| if c.is_ascii_alphanumeric() || "+-._?=".contains(c) { c } else { '_' }).collect();
    name.chars().take(200).collect()
}

// ---------- This Mac: its processor and macOS, as Homebrew names them ----------

/// macOS's versions by the names Homebrew's casks use for them.
const CODENAMES: &[((u32, u32), &str)] = &[
    ((10, 14), "mojave"),
    ((10, 15), "catalina"),
    ((11, 0), "big_sur"),
    ((12, 0), "monterey"),
    ((13, 0), "ventura"),
    ((14, 0), "sonoma"),
    ((15, 0), "sequoia"),
    ((26, 0), "tahoe"),
    ((27, 0), "golden_gate"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// Apple silicon (arm64), else Intel.
    pub arm: bool,
    /// macOS's version, (15, 1); (0, 0) when it isn't known.
    pub macos: (u32, u32),
}

impl Platform {
    pub fn here() -> Platform {
        static HERE: OnceLock<Platform> = OnceLock::new();
        *HERE.get_or_init(|| Platform { arm: cfg!(target_arch = "aarch64"), macos: macos_version().unwrap_or((0, 0)) })
    }

    /// macOS's name as Homebrew has it: `sequoia` (11 and later by their first number).
    fn codename(&self) -> Option<&'static str> {
        let key = if self.macos.0 >= 11 { (self.macos.0, 0) } else { self.macos };
        CODENAMES.iter().find(|(v, _)| *v == key).map(|(_, n)| *n)
    }

    /// The cask's variation for this Mac: `arm64_sequoia`, `sequoia` for Intel.
    pub fn tag(&self) -> Option<String> {
        let c = self.codename()?;
        Some(if self.arm { format!("arm64_{c}") } else { c.to_string() })
    }
}

/// This Mac's macOS (`sw_vers -productVersion`).
fn macos_version() -> Option<(u32, u32)> {
    if !crate::index::MAC {
        return None;
    }
    let out = Command::new("/usr/bin/sw_vers").arg("-productVersion").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    parse_macos(String::from_utf8_lossy(&out.stdout).trim())
}

/// A macOS version as casks write it: `14`, `10.15`, `15.1.1`, or its name (`:sonoma`).
pub fn parse_macos(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().trim_start_matches(':');
    if let Some((v, _)) = CODENAMES.iter().find(|(_, n)| *n == s) {
        return Some(*v);
    }
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map(|m| m.parse().unwrap_or(0)).unwrap_or(0);
    Some((major, minor))
}

/// Compared as macOS counts: from 11 by the first number alone, before it by two.
fn cmp_macos(a: (u32, u32), b: (u32, u32)) -> std::cmp::Ordering {
    if a.0 >= 11 || b.0 >= 11 {
        a.0.cmp(&b.0)
    } else {
        a.cmp(&b)
    }
}

fn shown_macos(v: (u32, u32)) -> String {
    if v.0 >= 11 && v.1 == 0 {
        v.0.to_string()
    } else {
        format!("{}.{}", v.0, v.1)
    }
}

// ---------- A cask, read ----------

/// A cask as Homebrew's JSON has it, for one Mac (its variation for that Mac applied): the
/// fields nami uses. Nothing in it is run: it's read as data.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Cask {
    pub token: String,
    #[serde(default)]
    pub name: Vec<String>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub url_specs: Option<Value>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<Value>,
    #[serde(default)]
    pub depends_on: Option<Value>,
    #[serde(default)]
    pub container: Option<Value>,
    #[serde(default)]
    pub auto_updates: Option<bool>,
    #[serde(default)]
    pub deprecated: Option<bool>,
    #[serde(default)]
    pub deprecation_reason: Option<String>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub disable_reason: Option<String>,
}

impl Cask {
    /// A cask's JSON as it is for `here`: its variation for this Mac (`arm64_sequoia`) over
    /// the rest, as Homebrew applies it.
    pub fn read(v: &Value, here: &Platform) -> Result<Cask> {
        let mut v = v.clone();
        if let (Some(tag), Some(Value::Object(vars))) = (here.tag(), v.get("variations").cloned()) {
            if let (Some(Value::Object(over)), Some(top)) = (vars.get(&tag), v.as_object_mut()) {
                for (k, x) in over {
                    top.insert(k.clone(), x.clone());
                }
            }
        }
        let cask: Cask = serde_json::from_value(v).context("a cask Homebrew's JSON has in a shape nami doesn't know")?;
        ensure!(valid_token(&cask.token), "{:?} isn't a cask's name", cask.token);
        Ok(cask)
    }

    /// The app's name as people know it: `Rectangle`.
    pub fn shown(&self) -> String {
        self.name.first().cloned().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| self.token.clone())
    }
}

/// An app a cask puts in Applications: where it is in the download, and its name there.
#[derive(Debug, Clone, PartialEq)]
pub struct AppPart {
    pub source: String,
    pub target: String,
}

/// A program inside the cask's app, for Terminal: `code` in the profile's bin, leading to the
/// app's copy in nami Apps (`app`, then `rest` inside it); a script that starts it when
/// `wrapper` (a cask's `command_wrapper`), else a link (`binary`).
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub name: String,
    pub app: String,
    pub rest: String,
    pub wrapper: bool,
}

/// What installing a cask comes to, for this Mac.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub token: String,
    pub name: String,
    pub version: String,
    pub url: String,
    /// None: the cask has no checksum (`no_check`).
    pub sha256: Option<String>,
    /// What the download is asked with (url_specs: a browser's User-Agent, a Referer...).
    pub headers: Vec<(String, String)>,
    /// An archive inside the download, unpacked in turn (`container: { nested: ... }`).
    pub nested: Option<String>,
    pub apps: Vec<AppPart>,
    pub links: Vec<Link>,
    pub auto_updates: bool,
    /// Said as it's installed (Homebrew's deprecated it).
    pub notes: Vec<String>,
}

/// The User-Agent `user_agent: :fake` asks for: a browser's (Homebrew's own).
const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

/// The end of every refusal that's about what the cask is.
const APP_ONLY: &str = "For now, nami installs Homebrew's casks that are an app and nothing more.";

/// What a cask's artifacts and stanzas are, said: what it needs that nami doesn't do.
fn needs(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "pkg" => "Apple's installer (a .pkg), which runs with an administrator's rights",
        "installer" => "its developer's own installer, run as it's installed",
        "suite" => "a folder of apps of its own (a suite)",
        "font" => "fonts",
        "qlplugin" => "a Quick Look plug-in",
        "prefpane" => "a System Settings pane",
        "screen_saver" => "a screen saver",
        "service" => "a Services menu item",
        "input_method" => "an input method",
        "keyboard_layout" => "a keyboard layout",
        "colorpicker" => "a color picker",
        "dictionary" => "a dictionary",
        "mdimporter" => "a Spotlight importer",
        "audio_unit_plugin" | "vst_plugin" | "vst3_plugin" => "an audio plug-in",
        "internet_plugin" => "a browser plug-in",
        "artifact" | "stage_only" => "files put outside Applications",
        "app_image" => "an AppImage",
        "preflight" | "postflight" | "preflight_steps" | "postflight_steps" => "steps run as it's installed",
        "binary" => "programs from its download outside its app, put in your PATH",
        _ => return None,
    })
}

/// What a cask has that nami leaves aside: what's only for removing it, and what's for
/// Terminal beside its programs (manual pages, completions).
const IGNORED: &[&str] = &[
    "uninstall", "zap", "uninstall_preflight", "uninstall_postflight", "uninstall_preflight_steps", "uninstall_postflight_steps", "manpage", "bash_completion",
    "zsh_completion", "fish_completion", "generate_completions_from_executable", "target",
];

/// A path in the download, as written (`Foo.app`, `Foo/Foo.app`): relative, and staying in it.
fn inside(p: &str) -> bool {
    !p.is_empty() && Path::new(p).components().all(|c| matches!(c, Component::Normal(_)))
}

/// An artifact's own options (`{ "target": "Bar.app" }`) after its source.
fn option<'a>(list: &'a [Value], key: &str) -> Option<&'a str> {
    list.iter().skip(1).find_map(|o| o.get(key)?.as_str())
}

/// What installing `c` comes to on `here`, or why nami won't (said for people).
pub fn plan(c: &Cask, here: &Platform) -> std::result::Result<Plan, String> {
    let name = c.shown();
    if c.disabled == Some(true) {
        let why = c.disable_reason.as_deref().map(|r| format!(" ({})", r.replace('_', " "))).unwrap_or_default();
        return Err(format!("{name} is disabled in Homebrew's casks{why}, so nami doesn't install it"));
    }
    let version = c.version.clone().filter(|v| !v.trim().is_empty()).ok_or_else(|| format!("{name}: Homebrew's cask has no version of it for this Mac"))?;
    let url = c.url.clone().filter(|u| !u.trim().is_empty()).ok_or_else(|| format!("{name}: Homebrew's cask has no download of it for this Mac"))?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("{name}'s download isn't a web address nami downloads ({})", url.split(':').next().unwrap_or_default()));
    }
    let sha256 = match c.sha256.as_deref().map(str::trim) {
        Some("no_check") => None,
        Some(s) if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) => Some(s.to_ascii_lowercase()),
        _ => return Err(format!("{name}: Homebrew's cask has no checksum nami can read")),
    };
    if sha256.is_none() && !url.starts_with("https://") {
        return Err(format!("{name}'s download has no checksum in Homebrew's cask and isn't on https, so nothing says it's the developer's"));
    }
    // What it needs.
    if let Some(dep) = c.depends_on.as_ref().and_then(Value::as_object) {
        for (k, v) in dep {
            match k.as_str() {
                "macos" => {
                    if let Some(why) = macos_needed(v, here) {
                        return Err(format!("{name} {version} needs {why}"));
                    }
                }
                "maximum_macos" => {
                    if let (Some(max), true) = (v.as_str().and_then(parse_macos), here.macos.0 > 0) {
                        if cmp_macos(here.macos, max).is_gt() {
                            return Err(format!("{name} {version} needs macOS {} or older; this Mac has macOS {}", shown_macos(max), shown_macos(here.macos)));
                        }
                    }
                }
                "arch" => {
                    let arches: Vec<&str> = v.as_array().into_iter().flatten().filter_map(|a| a.get("type")?.as_str()).collect();
                    let ours = if here.arm { "arm" } else { "intel" };
                    if !arches.is_empty() && !arches.contains(&ours) {
                        return Err(if here.arm { format!("{name} is for Intel Macs; this one has Apple silicon") } else { format!("{name} is for Macs with Apple silicon; this one has an Intel processor") });
                    }
                }
                "cask" | "formula" => {
                    let what: Vec<String> = match v {
                        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                        Value::String(s) => vec![s.clone()],
                        _ => Vec::new(),
                    };
                    if !what.is_empty() {
                        let kind = if k == "cask" { "cask" } else { "formula" };
                        return Err(format!("{name} needs Homebrew's {kind} {} too, which nami doesn't install. {APP_ONLY}", crate::tell::and_list(&what)));
                    }
                }
                _ => {}
            }
        }
    }
    // How it's downloaded.
    let mut headers = Vec::new();
    if let Some(specs) = c.url_specs.as_ref().and_then(Value::as_object) {
        for (k, v) in specs {
            match (k.as_str(), v) {
                ("user_agent", Value::String(ua)) => headers.push(("User-Agent".to_string(), match ua.as_str() { ":curl" => "curl/8.7.1".to_string(), u if u.starts_with(':') => BROWSER.to_string(), u => u.to_string() })),
                ("referer", Value::String(r)) => headers.push(("Referer".to_string(), r.clone())),
                ("cookies", Value::Object(m)) => headers.push(("Cookie".to_string(), m.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default())).collect::<Vec<_>>().join("; "))),
                ("header", h) => {
                    let lines: Vec<&str> = match h {
                        Value::String(s) => vec![s.as_str()],
                        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                        _ => Vec::new(),
                    };
                    for l in lines {
                        if let Some((k, v)) = l.split_once(':') {
                            headers.push((k.trim().to_string(), v.trim().to_string()));
                        }
                    }
                }
                ("using", Value::String(u)) if u.trim_start_matches(':') == "homebrew_curl" || u.trim_start_matches(':') == "curl" => {}
                ("verified", _) | ("using", Value::Null) => {}
                (other, _) => return Err(format!("{name}'s download needs what nami doesn't ask with ({other})")),
            }
        }
    }
    let nested = match c.container.as_ref() {
        None | Some(Value::Null) => None,
        Some(Value::Object(o)) => {
            if let Some(t) = o.get("type").and_then(Value::as_str) {
                if !matches!(t.trim_start_matches(':'), "zip" | "dmg" | "tar" | "gzip" | "bz2" | "xz") {
                    return Err(format!("{name}'s download is an archive nami doesn't unpack ({})", t.trim_start_matches(':')));
                }
            }
            match o.get("nested").and_then(Value::as_str) {
                Some(n) if inside(n) => Some(n.to_string()),
                Some(n) => return Err(format!("{name}: its archive inside the download is somewhere nami doesn't go ({n})")),
                None => None,
            }
        }
        Some(_) => None,
    };
    // What goes where.
    let (mut apps, mut wants, mut programs) = (Vec::new(), Vec::new(), Vec::new());
    for a in &c.artifacts {
        let Some(o) = a.as_object() else { continue };
        for (kind, v) in o {
            let list: Vec<Value> = match v {
                Value::Array(l) => l.clone(),
                Value::Null => Vec::new(),
                other => vec![other.clone()],
            };
            match kind.as_str() {
                "app" => {
                    let Some(source) = list.first().and_then(Value::as_str) else { continue };
                    if !inside(source) || !source.ends_with(".app") {
                        return Err(format!("{name}: its app is somewhere nami doesn't go ({source})"));
                    }
                    let target = option(&list, "target").map(|t| Path::new(t).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).unwrap_or_else(|| Path::new(source).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    if !target.ends_with(".app") || target.starts_with('.') || target.contains(['/', '\t', '\n']) {
                        return Err(format!("{name}: its app's name isn't one nami puts in Applications ({target})"));
                    }
                    apps.push(AppPart { source: source.to_string(), target });
                }
                "binary" => {
                    let Some(source) = list.first().and_then(Value::as_str) else { continue };
                    let target = option(&list, "target").map(String::from);
                    programs.push((source.to_string(), target, false));
                }
                "command_wrapper" => {
                    let (Some(name_), Some(exe)) = (list.first().and_then(Value::as_str), option(&list, "executable")) else { continue };
                    programs.push((exe.to_string(), Some(name_.to_string()), true));
                }
                k if IGNORED.contains(&k) => {}
                k => wants.push(needs(k).map(String::from).unwrap_or_else(|| format!("what Homebrew calls {k}"))),
            }
        }
    }
    if !wants.is_empty() {
        wants.dedup();
        return Err(format!("nami can't install {name} yet: it needs {}. {APP_ONLY}", crate::tell::and_list(&wants)));
    }
    if apps.is_empty() {
        return Err(format!("nami can't install {name} yet: it has no app to put in Applications. {APP_ONLY}"));
    }
    // Programs inside its app, for Terminal; any other's in the download alone.
    let mut links = Vec::new();
    for (source, target, wrapper) in programs {
        let within = source.strip_prefix("$APPDIR/").and_then(|rest| {
            let (app, inner) = rest.split_once(".app/")?;
            let app = format!("{app}.app");
            (apps.iter().any(|a| a.target == app) && inside(inner)).then(|| (app, inner.to_string()))
        });
        let Some((app, rest)) = within else {
            if wrapper {
                // (A wrapper of something not in its app: left aside, the app's whole without it.)
                continue;
            }
            return Err(format!("nami can't install {name} yet: it needs {}. {APP_ONLY}", needs("binary").unwrap_or_default()));
        };
        let program = target.as_deref().and_then(|t| Path::new(t).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| rest.rsplit('/').next().unwrap_or(&rest).to_string());
        if program.is_empty() || program.starts_with('.') || program.contains(['/', '\n', '\'']) {
            continue;
        }
        if !links.iter().any(|l: &Link| l.name == program) {
            links.push(Link { name: program, app, rest, wrapper });
        }
    }
    let mut notes = Vec::new();
    if c.deprecated == Some(true) {
        let why = c.deprecation_reason.as_deref().map(|r| format!(" ({})", r.replace('_', " "))).unwrap_or_default();
        notes.push(format!("Homebrew has deprecated its cask of {name}{why}: it may go from Homebrew's casks"));
    }
    Ok(Plan { token: c.token.clone(), name, version, url, sha256, headers, nested, apps, links, auto_updates: c.auto_updates == Some(true), notes })
}

/// Whether `here` is a macOS `depends_on.macos` takes; if not, what it needs.
fn macos_needed(v: &Value, here: &Platform) -> Option<String> {
    if here.macos.0 == 0 {
        return None;
    }
    let o = v.as_object()?;
    for (op, list) in o {
        let wanted: Vec<(u32, u32)> = match list {
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().and_then(parse_macos)).collect(),
            Value::String(s) => parse_macos(s).into_iter().collect(),
            _ => Vec::new(),
        };
        let Some(first) = wanted.first().copied() else { continue };
        let has = shown_macos(here.macos);
        let ord = cmp_macos(here.macos, first);
        let ok = match op.as_str() {
            ">=" => ord.is_ge(),
            ">" => ord.is_gt(),
            "<=" => ord.is_le(),
            "<" => ord.is_lt(),
            "==" => wanted.iter().any(|w| cmp_macos(here.macos, *w).is_eq()),
            _ => true,
        };
        if !ok {
            let needs = match op.as_str() {
                ">=" | ">" => format!("macOS {} or newer", shown_macos(first)),
                "<=" | "<" => format!("macOS {} or older", shown_macos(first)),
                _ => format!("macOS {}", wanted.iter().map(|w| shown_macos(*w)).collect::<Vec<_>>().join(" or ")),
            };
            return Some(format!("{needs}; this Mac has macOS {has}"));
        }
    }
    None
}

// ---------- The list of casks, kept ----------

/// A cask as the list keeps it, for this Mac: what search and info say, and whether nami can
/// install it here (`refusal`: why not).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub token: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default)]
    pub version: String,
    /// Installs with Homebrew in the last 30 days.
    #[serde(default)]
    pub installs: u64,
    #[serde(default)]
    pub auto_updates: bool,
    /// Its apps' names in Applications (`Rectangle.app`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// No checksum: notarized by Apple, or not installed.
    #[serde(default)]
    pub no_check: bool,
}

/// One cask of Homebrew's JSON as the list keeps it.
pub fn summarize(v: &Value, here: &Platform, installs: &BTreeMap<String, u64>) -> Option<Summary> {
    let c = Cask::read(v, here).ok()?;
    let plan = plan(&c, here);
    Some(Summary {
        name: c.shown(),
        desc: c.desc.clone().filter(|d| !d.trim().is_empty()),
        homepage: c.homepage.clone(),
        version: c.version.clone().unwrap_or_default(),
        installs: installs.get(&c.token).copied().unwrap_or(0),
        auto_updates: c.auto_updates == Some(true),
        apps: plan.as_ref().map(|p| p.apps.iter().map(|a| a.target.clone()).collect()).unwrap_or_default(),
        no_check: c.sha256.as_deref() == Some("no_check"),
        refusal: plan.err(),
        token: c.token,
    })
}

/// Homebrew's install counts (`analytics/cask-install/30d.json`): token -> installs.
pub fn read_installs(v: &Value) -> BTreeMap<String, u64> {
    v.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(|i| {
        let token = i.get("cask")?.as_str()?.to_string();
        let count = match i.get("count")? {
            Value::String(s) => s.replace(',', "").parse().ok()?,
            Value::Number(n) => n.as_u64()?,
            _ => return None,
        };
        Some((token, count))
    }).collect()
}

/// The list as it's kept: for which Mac, and when it was fetched.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    platform: String,
    fetched: u64,
    casks: Vec<Summary>,
}

fn cache_dir() -> Option<PathBuf> {
    if crate::profile::is_root() {
        // (nami's folder: /nami or /nix/var/nami, or a scratch root's under NAMI_ROOT.)
        return Some(crate::migrate::var_dir(&root()).join("casks"));
    }
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("nami/casks"))
}

fn api() -> String {
    std::env::var("NAMI_CASKS_API").ok().filter(|a| !a.is_empty()).unwrap_or_else(|| API.to_string())
}

fn http() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("nami/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// One of Homebrew's JSON answers (gzipped on the way when the server will).
fn fetch_json(url: &str) -> Result<Value> {
    let answer = http().get(url).set("Accept-Encoding", "gzip").call().map_err(|e| match e {
        ureq::Error::Status(code, _) => anyhow::anyhow!("Homebrew answered {code} for {url}"),
        ureq::Error::Transport(t) => anyhow::anyhow!("couldn't reach Homebrew ({t})"),
    })?;
    let gzipped = answer.header("Content-Encoding").is_some_and(|e| e.eq_ignore_ascii_case("gzip"));
    let mut data = Vec::new();
    let reader = answer.into_reader().take(MAX_JSON);
    if gzipped {
        flate2::read::GzDecoder::new(reader).take(MAX_JSON).read_to_end(&mut data)?;
    } else {
        let mut r = reader;
        r.read_to_end(&mut data)?;
    }
    serde_json::from_slice(&data).with_context(|| format!("reading {url}"))
}

/// Every cask, from Homebrew, as the list keeps them for this Mac (with their install counts,
/// when those come too).
fn fetch_list(here: &Platform) -> Result<Vec<Summary>> {
    let all = fetch_json(&format!("{}/cask.json", api()))?;
    let installs = fetch_json(&format!("{}/analytics/cask-install/30d.json", api())).map(|v| read_installs(&v)).unwrap_or_default();
    let list = all.as_array().context("Homebrew's list of casks isn't a list")?;
    Ok(list.iter().filter_map(|v| summarize(v, here, &installs)).collect())
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn list_file() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("list.json"))
}

fn read_kept() -> Option<Kept> {
    serde_json::from_slice(&std::fs::read(list_file()?).ok()?).ok()
}

fn keep(list: &[Summary], here: &Platform) {
    let Some(file) = list_file() else { return };
    let kept = Kept { platform: here.tag().unwrap_or_default(), fetched: now(), casks: list.to_vec() };
    if let (Some(dir), Ok(data)) = (file.parent(), serde_json::to_vec(&kept)) {
        let _ = std::fs::create_dir_all(dir);
        let part = file.with_extension("part");
        if std::fs::write(&part, data).is_ok() {
            let _ = std::fs::rename(&part, &file);
        }
    }
}

/// The list, fetched again now (`nami update`): how many casks there are.
pub fn refresh() -> Result<usize> {
    let here = Platform::here();
    let list = fetch_list(&here)?;
    keep(&list, &here);
    Ok(list.len())
}

/// The list of casks: as kept, fetched again when it's a day old (or kept for another macOS);
/// without the network, the one kept, however old. None: there's none (a Mac only fetches it).
pub fn list() -> Option<&'static [Summary]> {
    static LIST: OnceLock<Option<Vec<Summary>>> = OnceLock::new();
    LIST.get_or_init(|| {
        // (Unit tests don't ask Homebrew: what they check of the list, they give it.)
        if !crate::index::MAC || cfg!(test) {
            return None;
        }
        let here = Platform::here();
        let kept = read_kept();
        let fresh = kept.as_ref().is_some_and(|k| k.platform == here.tag().unwrap_or_default() && now().saturating_sub(k.fetched) < LIST_FOR.as_secs());
        if fresh {
            return kept.map(|k| k.casks);
        }
        match fetch_list(&here) {
            Ok(list) => {
                keep(&list, &here);
                Some(list)
            }
            Err(e) => {
                if kept.is_none() {
                    eprintln!("Homebrew's casks: {e:#}");
                }
                kept.map(|k| k.casks)
            }
        }
    })
    .as_deref()
}

/// A cask of the list, by its token.
pub fn find(token: &str) -> Option<&'static Summary> {
    static BY: OnceLock<BTreeMap<String, usize>> = OnceLock::new();
    let list = list()?;
    let by = BY.get_or_init(|| list.iter().enumerate().map(|(i, s)| (s.token.clone(), i)).collect());
    by.get(token).map(|&i| &list[i])
}

/// Whether there's a cask of that token (None: no list to tell by).
pub fn known(token: &str) -> Option<bool> {
    list().map(|_| find(token).is_some())
}

/// One cask's JSON, from Homebrew now (what's installed is its newest).
pub fn page(token: &str) -> Result<Value> {
    ensure!(valid_token(token), "{token:?} isn't a cask's name");
    fetch_json(&format!("{}/cask/{token}.json", api())).with_context(|| format!("asking Homebrew for its cask {token}"))
}

// ---------- Which of a file's packages are casks ----------

/// Whether a Mac's catalog has `pkg` ready to download (nothing's built on a Mac).
pub fn ready_built(pkg: &crate::index::Package) -> bool {
    pkg.cache.is_some() || (!pkg.uncached && pkg.closure.is_none())
}

/// A package of a nami file that's a cask here: the line (`cask:iterm2`, or `iterm2`), its
/// token, and why a plain name is the cask (said as it's installed).
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    pub listed: String,
    pub token: String,
    pub why: Option<String>,
}

/// Which of `packages` are casks on a Mac: every `cask:` entry, and a plain name nixpkgs has no
/// ready-built Mac build of when there's a cask of that name (`is_cask` says, from the list).
pub fn want(packages: &[String], index: Option<&crate::index::Index>, is_cask: impl Fn(&str) -> bool) -> Vec<Wanted> {
    let mut out = Vec::new();
    for p in packages {
        if let Some(t) = token(p) {
            out.push(Wanted { listed: p.clone(), token: t.to_string(), why: None });
            continue;
        }
        if !crate::config::is_plain_entry(p) || crate::index::split_version(p).1.is_some() || !valid_token(p) {
            continue;
        }
        let why = match index.map(|i| i.get(p)) {
            Some(Ok(pkg)) if ready_built(pkg) => continue,
            Some(Ok(_)) => "nixpkgs has no ready-built Mac build of it",
            _ => "nixpkgs has no package of that name for a Mac",
        };
        if is_cask(p) {
            out.push(Wanted { listed: p.clone(), token: p.clone(), why: Some(why.to_string()) });
        }
    }
    out
}

// ---------- Installing ----------

/// A cask as it's installed here: its entry, and the line nami.lock keeps.
pub struct Installed {
    pub entry: Entry,
    pub lock: crate::lock::Locked,
}

/// How far an apply moves the casks the lock has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Upgrade {
    /// As the lock has them.
    No,
    /// Each at the cask's newest (`nami upgrade`), but an app that updates itself.
    Newest,
    /// The same, but an app that's open now stays as it is, for the next day's (the daily
    /// update: nobody asked for it, and nobody's there to open it again).
    NewestClosed,
}

/// The casks a file lists, there: what the lock has (and the store still has) as it is, the
/// rest from Homebrew's cask now; with `upgrade`, each at the cask's newest, except an app that
/// updates itself (it's left as it is). `system`: everyone's (/Applications/nami Apps).
/// Those that can't be (refused, no network) come back with why, so the rest of a setup still
/// comes up.
pub fn install(store: &crate::store::Store, wanted: &[Wanted], old: &crate::lock::Lock, upgrade: Upgrade, locked: bool, system: bool) -> (Vec<Installed>, Vec<(String, String)>) {
    let (mut done, mut left) = (Vec::new(), Vec::new());
    let folder = crate::mac_apps::folder(system, store.root());
    for w in wanted {
        // (The lock's, under this name or the other the file may have had for it: `cask:x`, `x`.)
        let other = if w.listed == w.token { format!("{PREFIX}{}", w.token) } else { w.token.clone() };
        let had = [&w.listed, &other].into_iter().filter_map(|n| old.packages.get(n)).find(|l| l.name.starts_with(&format!("{PREFIX}{}-", w.token)) && !l.paths.is_empty() && l.paths.iter().all(|p| store.is_valid(crate::store::base_name(p))));
        if let Some(l) = had {
            let version = l.name.strip_prefix(&format!("{PREFIX}{}-", w.token)).unwrap_or_default().to_string();
            let newest = find(&w.token);
            let keep = kept(&version, newest, upgrade, || newest.zip(folder.as_deref()).is_some_and(|(s, f)| is_open(s, f)));
            match (&keep, newest) {
                (Keep::UpdatesItself, Some(s)) => crate::say!("{} updates itself, so nami upgrade leaves it at {version} (Homebrew's cask has {})", s.name, s.version),
                (Keep::Open, Some(s)) => crate::say!("{} is open, so it stays at {version} for now: {} comes once it's quit", s.name, s.version),
                _ => {}
            }
            if keep != Keep::Newer {
                done.push(Installed { entry: Entry { attr: w.listed.clone(), name: l.name.clone(), paths: l.paths.clone() }, lock: l.clone() });
                continue;
            }
        }
        if locked {
            left.push((w.listed.clone(), format!("nami.lock's {} isn't here, and {}", w.token, newest_only(&w.token))));
            continue;
        }
        match install_one(store, w, system) {
            Ok(i) => done.push(i),
            Err(e) => left.push((w.listed.clone(), format!("{e:#}"))),
        }
    }
    (done, left)
}

/// Whether one of the cask's apps (`s.apps`) is open now from nami Apps (`folder`): what
/// Spotlight, Launchpad and the Dock open is the copy there.
fn is_open(s: &Summary, folder: &Path) -> bool {
    s.apps.iter().any(|a| crate::mac_apps::is_open(&folder.join(a)))
}

/// What becomes of a cask the lock has (at `version`), the list's newest being `newest`.
#[derive(Debug, PartialEq)]
enum Keep {
    /// As it is: no upgrade, or it's the newest (or the newest isn't one nami installs).
    As,
    /// As it is, as the app updates itself (an upgrade doesn't replace it).
    UpdatesItself,
    /// As it is for now: the app's open, and the daily update doesn't replace it under it.
    Open,
    /// Homebrew's cask has a newer one, installed now.
    Newer,
}

/// (`open`: whether the app's open now, asked only when it matters.)
fn kept(version: &str, newest: Option<&Summary>, upgrade: Upgrade, open: impl FnOnce() -> bool) -> Keep {
    match newest {
        _ if upgrade == Upgrade::No => Keep::As,
        None => Keep::As,
        Some(s) if s.version == version || s.refusal.is_some() => Keep::As,
        Some(s) if s.auto_updates => Keep::UpdatesItself,
        Some(_) if upgrade == Upgrade::NewestClosed && open() => Keep::Open,
        Some(_) => Keep::Newer,
    }
}

/// The entries the casks listed would have, changing nothing (a dry run): the lock's, else
/// the cask's newest.
pub fn entries(wanted: &[Wanted], old: &crate::lock::Lock) -> Vec<Entry> {
    wanted.iter().map(|w| match old.packages.get(&w.listed).filter(|l| l.name.starts_with(PREFIX)) {
        Some(l) => Entry { attr: w.listed.clone(), name: l.name.clone(), paths: l.paths.clone() },
        None => Entry { attr: w.listed.clone(), name: entry_name(&w.token, find(&w.token).map(|s| s.version.as_str()).unwrap_or("newest")), paths: Vec::new() },
    }).collect()
}

/// Where downloads and what they unpack to wait: nami's own folder for root, else the person's
/// cache.
fn work_dir() -> Result<PathBuf> {
    cache_dir().context("no home folder to download into")
}

/// One cask, from its developer's download, into the store.
fn install_one(store: &crate::store::Store, w: &Wanted, system: bool) -> Result<Installed> {
    ensure!(crate::index::MAC, "Homebrew's casks are Mac apps");
    ensure!(valid_token(&w.token), "{:?} isn't a cask's name", w.token);
    let v = page(&w.token)?;
    let here = Platform::here();
    let cask = Cask::read(&v, &here)?;
    let plan = plan(&cask, &here).map_err(anyhow::Error::msg)?;
    if let Some(why) = &w.why {
        crate::say!("{}: {} {FROM} ({why})", w.listed, plan.name);
    }
    for n in &plan.notes {
        crate::say!("warning: {n}");
    }
    let folder = crate::mac_apps::folder(system, store.root()).context("no home folder for nami Apps")?;
    let work = work_dir()?.join(format!("{}-{}", plan.token, std::process::id()));
    if work.symlink_metadata().is_ok() {
        crate::store::remove_tree(&work)?;
    }
    std::fs::create_dir_all(&work).with_context(|| format!("can't make {}", work.display()))?;
    let made = (|| -> Result<String> {
        let file = download(&plan, &work.join("download"))?;
        let unpacked = work.join("unpacked");
        unpack(&file, &unpacked, &work)?;
        let _ = std::fs::remove_file(&file);
        let root = match &plan.nested {
            Some(n) => {
                let inner = work.join("nested");
                unpack(&unpacked.join(n), &inner, &work).with_context(|| format!("unpacking {n}, in {}'s download", plan.name))?;
                inner
            }
            None => unpacked,
        };
        let root = inner_disk_image(&plan, root, &work)?;
        let stage = work.join("stage");
        stage_apps(&plan, &root, &stage, &folder)?;
        for a in &plan.apps {
            check(&plan, &stage.join("Applications").join(&a.target))?;
        }
        // The apps in nami's casks folder, on the Mac's own volume (`keep_apps`); the store path has
        // links to them, and the programs for Terminal.
        // Whether each app's signature holds as its developer made it (Gatekeeper's: macOS stops
        // an app on its first open without one it accepts, whatever nami does).
        let signed: Vec<bool> = plan.apps.iter().map(|a| !crate::index::MAC || codesign_verify(&stage.join("Applications").join(&a.target)).is_ok()).collect();
        let name = store_name(&plan.token, &plan.version);
        let kept = keep_apps(store, &stage.join("Applications"), &name, &work)?;
        link_apps(&plan, &stage, &kept)?;
        let path = store.add_path(&stage, &name)?;
        if crate::index::MAC {
            for (a, signed) in plan.apps.iter().zip(&signed) {
                match codesign_verify(&kept.join(&a.target)) {
                    Ok(()) => {}
                    // nami's copy lost what the developer's had: macOS would refuse it.
                    Err(why) if *signed => bail!("{}'s signature didn't hold in nami's copy ({why}), so it isn't installed. Please tell us: nami report", plan.name),
                    Err(_) => crate::say!(
                        "{} isn't signed in a way macOS accepts, so macOS stops it the first time it opens. To open it anyway: System Settings › Privacy & Security, then Open Anyway.",
                        plan.name
                    ),
                }
            }
        }
        Ok(path)
    })();
    let _ = crate::store::remove_tree(&work);
    let path = made?;
    let name = entry_name(&plan.token, &plan.version);
    let entry = Entry { attr: w.listed.clone(), name: name.clone(), paths: vec![path.clone()] };
    Ok(Installed { entry, lock: crate::lock::Locked { name, paths: vec![path], rev: None, inputs: Vec::new() } })
}

/// A download that's a disk image inside a zip the cask doesn't say so of (Homebrew opens it all
/// the same): when the apps the cask names aren't in what was unpacked, and the one disk image
/// at its top (or one folder down) is, that disk image's files. Else what was unpacked.
fn inner_disk_image(plan: &Plan, root: PathBuf, work: &Path) -> Result<PathBuf> {
    if plan.apps.iter().all(|a| root.join(&a.source).symlink_metadata().is_ok()) {
        return Ok(root);
    }
    let images = disk_images_in(&root);
    let [image] = images.as_slice() else { return Ok(root) };
    let inner = work.join("inner");
    unpack(image, &inner, work).with_context(|| format!("opening {}, in {}'s download", image.file_name().unwrap_or_default().to_string_lossy(), plan.name))?;
    Ok(inner)
}

/// The disk images (.dmg) at `root`'s top and one folder down, not in an app.
fn disk_images_in(root: &Path) -> Vec<PathBuf> {
    let is_dmg = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dmg")) && p.is_file();
    let mut found = Vec::new();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let p = e.path();
        if is_dmg(&p) {
            found.push(p);
        } else if p.is_dir() && !p.extension().is_some_and(|e| e == "app") && p.file_name().is_some_and(|n| n != "__MACOSX") {
            found.extend(std::fs::read_dir(&p).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_dmg(p)));
        }
    }
    found
}

/// The cask's download, into `dest`, checked against its sha256.
fn download(plan: &Plan, dest: &Path) -> Result<PathBuf> {
    let mut req = http().get(&plan.url);
    for (k, v) in &plan.headers {
        req = req.set(k, v);
    }
    let answer = req.call().map_err(|e| match e {
        ureq::Error::Status(code, _) => anyhow::anyhow!("{}'s download answered {code} ({})", plan.name, plan.url),
        ureq::Error::Transport(t) => anyhow::anyhow!("couldn't download {} ({t})", plan.name),
    })?;
    // No checksum: the download is the developer's only as Gatekeeper says (`check`), so
    // nothing on the way may be in the clear either.
    if plan.sha256.is_none() {
        ensure!(answer.get_url().starts_with("https://"), "{}'s download went to an address that isn't https ({}), and the cask has no checksum", plan.name, answer.get_url());
    }
    let host = answer.get_url().split('/').nth(2).unwrap_or_default().to_string();
    let size = answer.header("Content-Length").and_then(|l| l.parse::<u64>().ok());
    let shown = size.map(|s| format!(", {}", crate::tell::size(s))).unwrap_or_default();
    // (Not a `…` line: those are a terminal's alone, and this is said in a pipe too.)
    crate::say!("Downloading {} {} from {host}{shown}", plan.name, plan.version);
    let mut file = std::io::BufWriter::new(std::fs::File::create(dest).with_context(|| format!("can't write {}", dest.display()))?);
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    let mut reader = answer.into_reader().take(MAX_DOWNLOAD);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut buf).with_context(|| format!("downloading {}", plan.name))?;
        if n == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buf[..n]);
        file.write_all(&buf[..n])?;
    }
    file.flush()?;
    let got: String = sha2::Digest::finalize(hasher).iter().map(|b| format!("{b:02x}")).collect();
    if let Some(want) = &plan.sha256 {
        ensure!(got == *want, "{}'s download isn't what Homebrew's cask says it is (its sha256 is {got}, the cask's {want}), so nami didn't install it", plan.name);
    }
    Ok(dest.to_path_buf())
}

/// What a download is, by its first (or last) bytes.
#[derive(Debug, PartialEq)]
enum Kind {
    Zip,
    Tar,
    Dmg,
    Pkg,
    Other,
}

fn kind_of(file: &Path) -> Result<Kind> {
    let mut f = std::fs::File::open(file).with_context(|| format!("reading {}", file.display()))?;
    let mut head = [0u8; 512];
    let n = f.read(&mut head)?;
    let head = &head[..n];
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Ok(Kind::Zip);
    }
    if head.starts_with(b"xar!") {
        return Ok(Kind::Pkg);
    }
    // A disk image: its trailer, `koly`, in the last 512 bytes. First: its first bytes are its
    // first block's, compressed (bzip2's `BZh` in Chrome's), as a compressed tar's would be.
    use std::io::{Seek, SeekFrom};
    let len = f.metadata()?.len();
    if len >= 512 {
        f.seek(SeekFrom::Start(len - 512))?;
        let mut tail = [0u8; 4];
        f.read_exact(&mut tail)?;
        if &tail == b"koly" {
            return Ok(Kind::Dmg);
        }
    }
    if head.starts_with(&[0x1f, 0x8b]) || head.starts_with(b"BZh") || head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) || head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) || head.get(257..262) == Some(b"ustar") {
        return Ok(Kind::Tar);
    }
    Ok(Kind::Other)
}

/// Unpack a download (a zip, a tar, a disk image) into `dest`. On a Mac with its own tools, as
/// Finder would (`ditto`, `tar`, `hdiutil`); elsewhere (tests) zips and tars by nami's own.
fn unpack(file: &Path, dest: &Path, work: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let kind = kind_of(file)?;
    let run = |cmd: &mut Command, what: &str| -> Result<()> {
        let out = cmd.stdin(Stdio::null()).output().with_context(|| format!("running {what}"))?;
        ensure!(out.status.success(), "{what} couldn't unpack it: {}", String::from_utf8_lossy(&out.stderr).trim());
        Ok(())
    };
    match kind {
        Kind::Zip if crate::index::MAC => run(Command::new("/usr/bin/ditto").arg("-x").arg("-k").arg(file).arg(dest), "ditto"),
        Kind::Tar if crate::index::MAC => run(Command::new("/usr/bin/tar").arg("-xf").arg(file).arg("-C").arg(dest), "tar"),
        Kind::Zip | Kind::Tar => crate::archive::unpack_bytes(&std::fs::read(file)?, dest, false),
        Kind::Dmg if crate::index::MAC => from_disk_image(file, dest, work),
        Kind::Dmg => bail!("a disk image is unpacked on a Mac"),
        Kind::Pkg => bail!("the download is an installer package (.pkg), not an app. {APP_ONLY}"),
        Kind::Other => bail!("the download isn't a zip, a tar or a disk image (.dmg), which is what nami unpacks"),
    }
}

/// A disk image's files, copied out (`hdiutil attach -readonly -nobrowse`, `ditto`, detach).
/// One that asks to agree to a licence first is converted and attached again, as Homebrew does.
fn from_disk_image(file: &Path, dest: &Path, work: &Path) -> Result<()> {
    let mounts = work.join("mnt");
    std::fs::create_dir_all(&mounts)?;
    let attach = |image: &Path| -> std::io::Result<std::process::Output> {
        let mut child = Command::new("/usr/bin/hdiutil")
            .args(["attach", "-plist", "-nobrowse", "-readonly", "-noautoopen", "-noverify", "-mountrandom"])
            .arg(&mounts)
            .arg(image)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // (A licence's pager: quit it, and don't agree; a convert then.)
        if let Some(mut i) = child.stdin.take() {
            let _ = i.write_all(b"qn\n");
        }
        child.wait_with_output()
    };
    let mut out = attach(file).context("running hdiutil")?;
    if !out.status.success() {
        let cdr = work.join("image.cdr");
        let conv = Command::new("/usr/bin/hdiutil").args(["convert", "-quiet", "-format", "UDTO", "-o"]).arg(&cdr).arg(file).stdin(Stdio::null()).output().context("running hdiutil")?;
        ensure!(conv.status.success(), "hdiutil couldn't open the disk image: {}", String::from_utf8_lossy(&out.stderr).trim());
        out = attach(&cdr).context("running hdiutil")?;
        ensure!(out.status.success(), "hdiutil couldn't open the disk image: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let points = mount_points(&String::from_utf8_lossy(&out.stdout));
    ensure!(!points.is_empty(), "the disk image has nothing to open");
    let mut copied = Ok(());
    for p in &points {
        if copied.is_ok() {
            let o = Command::new("/usr/bin/ditto").arg(p).arg(dest).stdin(Stdio::null()).output().context("running ditto");
            copied = match o {
                Ok(o) if o.status.success() => Ok(()),
                Ok(o) => Err(anyhow::anyhow!("couldn't copy from the disk image: {}", String::from_utf8_lossy(&o.stderr).trim())),
                Err(e) => Err(e),
            };
        }
    }
    for p in &points {
        let gone = Command::new("/usr/bin/hdiutil").arg("detach").arg(p).arg("-quiet").stdin(Stdio::null()).status().is_ok_and(|s| s.success());
        if !gone {
            let _ = Command::new("/usr/bin/hdiutil").arg("detach").arg(p).arg("-force").arg("-quiet").stdin(Stdio::null()).status();
        }
    }
    copied
}

/// The mount points in what `hdiutil attach -plist` says.
fn mount_points(plist: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut rest = plist;
    while let Some(i) = rest.find("<key>mount-point</key>") {
        rest = &rest[i + "<key>mount-point</key>".len()..];
        let Some(open) = rest.find("<string>") else { break };
        let Some(close) = rest[open..].find("</string>") else { break };
        let p = &rest[open + "<string>".len()..open + close];
        out.push(PathBuf::from(p.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&apos;", "'").replace("&quot;", "\"")));
        rest = &rest[open + close..];
    }
    out
}

/// The store path's tree: the cask's apps in `Applications/`, under their names there, and its
/// programs in `bin/`, leading to the apps' copies in `folder` (nami Apps): what Terminal runs
/// is the app Spotlight opens, and updates with it when the app updates itself.
fn stage_apps(plan: &Plan, root: &Path, stage: &Path, folder: &Path) -> Result<()> {
    let apps = stage.join("Applications");
    std::fs::create_dir_all(&apps)?;
    for a in &plan.apps {
        let from = root.join(&a.source);
        let found = from.symlink_metadata().is_ok_and(|m| m.is_dir());
        ensure!(found, "{}'s download has no {}", plan.name, a.source);
        let to = apps.join(&a.target);
        ensure!(to.symlink_metadata().is_err(), "{} has two apps named {}", plan.name, a.target);
        std::fs::rename(&from, &to).with_context(|| format!("moving {}", a.source))?;
    }
    if plan.links.is_empty() {
        return Ok(());
    }
    let bin = stage.join("bin");
    std::fs::create_dir_all(&bin)?;
    for l in &plan.links {
        if !apps.join(&l.app).join(&l.rest).exists() {
            crate::say!("warning: {} has no {} in {}, so there's no {} command", plan.name, l.rest, l.app, l.name);
            continue;
        }
        let there = folder.join(&l.app).join(&l.rest);
        if l.wrapper {
            let script = format!("#!/bin/sh\nexec '{}' \"$@\"\n", there.display().to_string().replace('\'', "'\\''"));
            std::fs::write(bin.join(&l.name), script)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join(&l.name), std::fs::Permissions::from_mode(0o755))?;
        } else {
            std::os::unix::fs::symlink(&there, bin.join(&l.name))?;
        }
    }
    Ok(())
}

// ---------- Where a cask's apps are kept: on the Mac's own volume ----------
//
// /nix is a volume of its own, and APFS clones only within one: an app kept in the store and
// copied into nami Apps would take its size twice (Chrome: a 790 MB store path and a 754 MB copy). So a
// cask's apps are kept in /Library/Application Support/nami/casks/<token>-<version>-<hash>/,
// root's, on the Data volume as /Users and /Applications are (firmlinks), and nami Apps has
// clones of them (mac_apps.rs: clonefile), which share their blocks: one app's size for both.
// The store path keeps the rest: links to them in its Applications/, the programs in its bin/.
// A folder stays while a store path links to it (any step of anyone's that has the cask), and
// `nami gc` takes those nothing links to (`sweep`, from Store::gc_with).

/// nami's folder of casks' apps, under `root` (`/` on a Mac).
pub const DIR: &str = "Library/Application Support/nami/casks";

pub fn dir(root: &Path) -> PathBuf {
    root.join(DIR)
}

/// The apps in `apps` (a folder of `<App>.app`), into nami's casks folder: as root, put there; as
/// anyone else, through the daemon (the folder is root's). Its name is the store name's
/// (`cask-rectangle-2.0.5`, less `cask-`) and the start of its archive's hash, so one of other
/// contents is another folder. Returns the folder.
fn keep_apps(store: &crate::store::Store, apps: &Path, name: &str, work: &Path) -> Result<PathBuf> {
    let signatures = take_signatures(apps)?;
    if !signatures.is_empty() {
        std::fs::write(apps.join(SIGNATURES), serde_json::to_vec(&signatures)?)?;
    }
    let nar = work.join("apps.nar");
    let mut w = crate::nar::HashingWriter::new(std::io::BufWriter::new(std::fs::File::create(&nar)?));
    crate::nar::dump(apps, &mut w).with_context(|| format!("archiving {}", apps.display()))?;
    w.flush()?;
    let hash = w.finish();
    let kept = match store.daemon() {
        Some(socket) => crate::daemon::client::add_cask(socket, &nar, name).map(PathBuf::from),
        None => add_folder(store.root(), &nar, &hash, name),
    };
    let _ = std::fs::remove_file(&nar);
    kept
}

/// Signatures macOS keeps beside a file, not in it: `com.apple.cs.*` extended attributes, on what
/// isn't a program in a bundle's code places (VLC's plug-in .jar, Slack's sounds). An archive (a
/// nar) has no room for them, and without them macOS calls the app unsigned ("can't be opened,
/// because the developer can't be verified"). So they go in a file of their own at the archive's
/// top (`SIGNATURES`: each one's path in it, name and value), put back as it's unpacked.
const SIGNATURES: &str = ".nami-signatures";
const SIGNATURE_PREFIX: &str = "com.apple.cs.";
/// The most a signature may be, and all of an app's together.
const SIGNATURE_MAX: usize = 1 << 20;
const SIGNATURES_MAX: usize = 64 << 20;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Signature {
    path: String,
    name: String,
    /// base64.
    value: String,
}

/// The signatures in `apps` (a cask's staged Applications folder), each file's and folder's;
/// none off a Mac.
fn take_signatures(apps: &Path) -> Result<Vec<Signature>> {
    let mut found = Vec::new();
    #[cfg(target_os = "macos")]
    {
        use base64::Engine;
        let mut todo = vec![apps.to_path_buf()];
        while let Some(p) = todo.pop() {
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                for e in std::fs::read_dir(&p)? {
                    todo.push(e?.path());
                }
            }
            if p == apps {
                continue;
            }
            let rel = p.strip_prefix(apps)?.to_string_lossy().into_owned();
            for name in xattr_names(&p)? {
                if name.starts_with(SIGNATURE_PREFIX) {
                    let value = xattr_get(&p, &name)?;
                    found.push(Signature { path: rel.clone(), name, value: base64::engine::general_purpose::STANDARD.encode(value) });
                }
            }
        }
    }
    let _ = apps;
    Ok(found)
}

/// Where a signature goes in `folder`, and its value: inside it (no `..`, not through a link), a
/// signature's name, and not too big. (The daemon puts them back as root, from a user's archive.)
fn signature_place(folder: &Path, s: &Signature, total: &mut usize) -> Result<(PathBuf, String, Vec<u8>)> {
    use base64::Engine;
    let rel = Path::new(&s.path);
    ensure!(!s.path.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_))), "a signature's path isn't inside the app: {:?}", s.path);
    ensure!(s.name.starts_with(SIGNATURE_PREFIX) && s.name.len() <= 127 && !s.name.contains('\0') && !s.name.contains('/'), "not a signature: {:?}", s.name);
    let value = base64::engine::general_purpose::STANDARD.decode(&s.value).context("a signature's value isn't base64")?;
    ensure!(value.len() <= SIGNATURE_MAX, "a signature of {} bytes", value.len());
    *total += value.len();
    ensure!(*total <= SIGNATURES_MAX, "more signatures than an app has");
    // Each folder on the way, and the file, as they are: none a link.
    let mut at = folder.to_path_buf();
    for c in rel.components() {
        at.push(c);
        let m = std::fs::symlink_metadata(&at).with_context(|| format!("a signature for {}, which isn't there", s.path))?;
        ensure!(!m.file_type().is_symlink(), "a signature for {}, through a link", s.path);
    }
    Ok((at, s.name.clone(), value))
}

/// The signatures `folder`'s archive brought (`SIGNATURES`), put back on its files; that file goes.
fn put_signatures(folder: &Path) -> Result<()> {
    let file = folder.join(SIGNATURES);
    let Ok(text) = std::fs::read(&file) else { return Ok(()) };
    let _ = std::fs::remove_file(&file);
    let list: Vec<Signature> = serde_json::from_slice(&text).context("the app's signatures")?;
    let mut total = 0;
    for s in &list {
        let (at, name, value) = signature_place(folder, s, &mut total)?;
        #[cfg(target_os = "macos")]
        xattr_set(&at, &name, &value).with_context(|| format!("putting back {}'s signature", s.path))?;
        let _ = (&at, &name, &value);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn c_path(p: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(std::ffi::CString::new(p.as_os_str().as_bytes())?)
}

/// A file's extended attributes' names (not following a link).
#[cfg(target_os = "macos")]
fn xattr_names(p: &Path) -> Result<Vec<String>> {
    let c = c_path(p)?;
    // SAFETY: c is NUL-terminated; a null buffer of size 0 asks for the size alone.
    let n = unsafe { libc::listxattr(c.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW) };
    if n <= 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; n as usize];
    // SAFETY: buf is buf.len() bytes, which listxattr writes no more than.
    let n = unsafe { libc::listxattr(c.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), libc::XATTR_NOFOLLOW) };
    ensure!(n >= 0, "listing {}'s attributes: {}", p.display(), std::io::Error::last_os_error());
    buf.truncate(n as usize);
    Ok(buf.split(|&b| b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).into_owned()).collect())
}

#[cfg(target_os = "macos")]
fn xattr_get(p: &Path, name: &str) -> Result<Vec<u8>> {
    let (c, cn) = (c_path(p)?, std::ffi::CString::new(name)?);
    // SAFETY: both NUL-terminated; a null buffer of size 0 asks for the size alone.
    let n = unsafe { libc::getxattr(c.as_ptr(), cn.as_ptr(), std::ptr::null_mut(), 0, 0, libc::XATTR_NOFOLLOW) };
    ensure!(n >= 0, "reading {} of {}: {}", name, p.display(), std::io::Error::last_os_error());
    ensure!(n as usize <= SIGNATURE_MAX, "{} of {} is {} bytes", name, p.display(), n);
    let mut buf = vec![0u8; n as usize];
    // SAFETY: buf is buf.len() bytes, which getxattr writes no more than.
    let n = unsafe { libc::getxattr(c.as_ptr(), cn.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, libc::XATTR_NOFOLLOW) };
    ensure!(n >= 0, "reading {} of {}: {}", name, p.display(), std::io::Error::last_os_error());
    buf.truncate(n as usize);
    Ok(buf)
}

#[cfg(target_os = "macos")]
fn xattr_set(p: &Path, name: &str, value: &[u8]) -> Result<()> {
    let (c, cn) = (c_path(p)?, std::ffi::CString::new(name)?);
    // SAFETY: both NUL-terminated, and value is value.len() bytes.
    let rc = unsafe { libc::setxattr(c.as_ptr(), cn.as_ptr(), value.as_ptr().cast(), value.len(), 0, libc::XATTR_NOFOLLOW) };
    ensure!(rc == 0, "{}", std::io::Error::last_os_error());
    Ok(())
}

/// An archive of a cask's apps (`nar`, its hash `nar_hash`), unpacked into nami's casks folder
/// (root's: the daemon's, or root's own nami), unless it's there. Read-only, as the store is.
pub fn add_folder(root: &Path, nar: &Path, nar_hash: &str, name: &str) -> Result<PathBuf> {
    let token_version = name.strip_prefix("cask-").filter(|_| crate::store::valid_store_name(name)).with_context(|| format!("{name:?} isn't a cask's name"))?;
    let short: String = nar_hash.trim_start_matches("sha256:").chars().filter(char::is_ascii_alphanumeric).take(12).collect();
    ensure!(short.len() == 12, "a cask's archive without its hash");
    let dir = dir(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("can't make {}", dir.display()))?;
    let folder = dir.join(format!("{token_version}-{short}"));
    if folder.is_dir() {
        return Ok(folder);
    }
    let new = dir.join(format!(".new-{}-{token_version}", std::process::id()));
    if new.symlink_metadata().is_ok() {
        crate::store::remove_tree(&new)?;
    }
    crate::nar::unpack(&mut std::io::BufReader::new(std::fs::File::open(nar)?), &new).with_context(|| format!("unpacking into {}", new.display()))?;
    if let Err(e) = put_signatures(&new) {
        let _ = crate::store::remove_tree(&new);
        return Err(e);
    }
    // (Its time is when it was kept, which `sweep` spares a while: an archive's has none.)
    let _ = std::fs::File::open(&new).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    // (Writable while it's renamed: a Mac refuses to move a folder its owner can't write to, as
    // the archive's are, unless it's root's. Then read-only again.)
    use std::os::unix::fs::PermissionsExt;
    let mode = |m: u32| std::fs::Permissions::from_mode(m);
    let _ = std::fs::set_permissions(&new, mode(0o755));
    if let Err(e) = std::fs::rename(&new, &folder) {
        let _ = crate::store::remove_tree(&new);
        if !folder.is_dir() {
            return Err(e).with_context(|| format!("can't put {}", folder.display()));
        }
        return Ok(folder);
    }
    let _ = std::fs::set_permissions(&folder, mode(0o555));
    Ok(folder)
}

/// The stage's apps as links to where they're kept (`kept`/<App>.app).
fn link_apps(plan: &Plan, stage: &Path, kept: &Path) -> Result<()> {
    let apps = stage.join("Applications");
    crate::store::remove_tree(&apps)?;
    std::fs::create_dir_all(&apps)?;
    for a in &plan.apps {
        std::os::unix::fs::symlink(kept.join(&a.target), apps.join(&a.target))?;
    }
    Ok(())
}

/// A folder in nami's casks folder nothing may link to yet (an install between keeping its apps
/// and adding its store path) is spared this long.
const SPARE_FOR: u64 = 3600;

/// Take away the cask folders no store path nami keeps links to (what `Store::gc_with` left is
/// what's in use: any step of any profile's). How many went, and their size.
pub fn sweep(store: &crate::store::Store) -> (usize, u64) {
    let dir = dir(store.root());
    let Ok(entries) = std::fs::read_dir(&dir) else { return (0, 0) };
    // What the store's casks link to.
    let mut live = std::collections::BTreeSet::new();
    for e in std::fs::read_dir(store.var_dir().join("db")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(base) = name.strip_suffix(".json").filter(|b| b.contains("-cask-")) else { continue };
        let apps = store.phys(&format!("{}/{base}/Applications", crate::store::STORE_DIR));
        for a in std::fs::read_dir(&apps).into_iter().flatten().flatten() {
            if let Ok(to) = std::fs::read_link(a.path()) {
                if let Some(folder) = to.strip_prefix(&dir).ok().and_then(|r| r.components().next()) {
                    live.insert(folder.as_os_str().to_os_string());
                }
            }
        }
    }
    let (mut n, mut size) = (0, 0);
    for e in entries.flatten() {
        let young = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < SPARE_FOR);
        if live.contains(&e.file_name()) || young {
            continue;
        }
        let bytes = size_of(&e.path());
        if crate::store::remove_tree(&e.path()).is_ok() {
            n += 1;
            size += bytes;
        }
    }
    (n, size)
}

/// A folder's files' size.
fn size_of(p: &Path) -> u64 {
    match p.symlink_metadata() {
        Ok(m) if m.is_dir() => std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| size_of(&e.path())).sum(),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// What Gatekeeper says of an app.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// Signed with a Developer ID and notarized by Apple.
    Notarized,
    /// Accepted otherwise (its source, as spctl says it).
    Accepted(String),
    Rejected(String),
}

/// Whether nami installs the app: with a checksum, any (and macOS asks the first time, as for
/// any download; one Apple hasn't notarized it says so); without one, only one Gatekeeper
/// accepts as notarized.
fn check(plan: &Plan, app: &Path) -> Result<()> {
    if !crate::index::MAC {
        return Ok(());
    }
    let verdict = assess(app);
    match (&verdict, plan.sha256.is_some()) {
        (Verdict::Notarized, _) => Ok(()),
        (_, false) => bail!(
            "Homebrew's cask of {} has no checksum (its developer's link always has the newest), so nami installs it only if Apple has notarized it, and Gatekeeper says: {}",
            plan.name,
            match verdict {
                Verdict::Accepted(s) => format!("accepted, from {s}, not notarized"),
                Verdict::Rejected(why) => why,
                Verdict::Notarized => unreachable!(),
            }
        ),
        (Verdict::Accepted(_), true) => Ok(()),
        (Verdict::Rejected(why), true) => {
            crate::say!("warning: Gatekeeper doesn't accept {} ({why}): macOS will block it the first time. To open it then: System Settings > Privacy & Security > Open Anyway.", plan.name);
            Ok(())
        }
    }
}

/// Gatekeeper's assessment (`spctl --assess --type execute`). With Gatekeeper turned off, the
/// same rule by codesign: a Developer ID's signature that Apple has notarized.
pub fn assess(app: &Path) -> Verdict {
    let out = Command::new("/usr/sbin/spctl").args(["--assess", "--type", "execute", "-vv"]).arg(app).stdin(Stdio::null()).output();
    let Ok(out) = out else { return Verdict::Rejected("spctl couldn't run".into()) };
    let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    read_assessment(out.status.success(), &text, app).unwrap_or_else(|| {
        // (Assessments are off on this Mac: codesign checks the same rule.)
        let rule = "=notarized and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists";
        let o = Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict", "-R", rule]).arg(app).stdin(Stdio::null()).output();
        match o {
            Ok(o) if o.status.success() => Verdict::Notarized,
            Ok(o) => Verdict::Rejected(format!("Gatekeeper is off, and codesign says: {}", last_line(&String::from_utf8_lossy(&o.stderr), app))),
            Err(_) => Verdict::Rejected("codesign couldn't run".into()),
        }
    })
}

/// spctl's answer: None when it didn't assess (Gatekeeper's off: `override=security disabled`).
fn read_assessment(ok: bool, text: &str, app: &Path) -> Option<Verdict> {
    if text.contains("override=security disabled") {
        return None;
    }
    let source = text.lines().find_map(|l| l.trim().strip_prefix("source=")).map(str::trim);
    Some(match (ok, source) {
        (true, Some("Notarized Developer ID")) => Verdict::Notarized,
        (true, s) => Verdict::Accepted(s.unwrap_or("an unknown source").to_string()),
        (false, _) => Verdict::Rejected(last_line(text, app)),
    })
}

/// What a tool's last line says, without the path it starts with.
fn last_line(text: &str, app: &Path) -> String {
    let at = format!("{}: ", app.display());
    let line = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("source=") && !l.starts_with("origin=")).last().unwrap_or("rejected");
    line.strip_prefix(&at).unwrap_or(line).to_string()
}

/// `codesign --verify --deep --strict`: whether the app's signature holds.
fn codesign_verify(app: &Path) -> std::result::Result<(), String> {
    let o = Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(app).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(last_line(&String::from_utf8_lossy(&o.stderr), app))
    }
}

// ---------- Icons ----------
//
// A cask's icon is its app's (Homebrew's JSON has none), from one of two places:
// - The set made for every cask nami installs (packaging/cask-icons.py, on GitHub's runners,
//   from each cask's download; docs/macos.md). Each icon's sha256 is in the signed Mac catalog,
//   in an entry of its own (`ICONS_ENTRY`). The most installed (`cask_icons`) come with
//   `nami update`, as the catalog's own icons do (index.rs: only what changed), into nami's icons
//   folder as `cask:<token>.png`; an icon the catalog doesn't name isn't kept. The rest
//   (`cask_icons_later`) each as the app is first shown (search, info: `later_icons`), from the
//   catalog's objects, kept in the cache once it's the sha256 the catalog names.
// - An installed cask the set hasn't got: its app's own icon (its Info.plist's, made a PNG by
//   `sips`), once, in the person's cache.

/// The catalog's entry that names the set's icons (`cask_icons`: token -> sha256). It isn't a
/// package: no outputs, and not ready-built (`uncached`), so no nami offers to install it; a nami
/// from before it reads it as such and leaves the icons be.
pub const ICONS_ENTRY: &str = "nami:cask-icons";

#[derive(Deserialize)]
struct IconSet {
    #[serde(default)]
    cask_icons: BTreeMap<String, String>,
    /// The other casks' icons, fetched as each is first shown: token -> sha256.
    #[serde(default)]
    cask_icons_later: BTreeMap<String, String>,
    /// The Mac App Store's screenshots of the casks' apps it sells: token -> their sha256s,
    /// each one in `menu/objects/`.
    #[serde(default)]
    cask_screenshots: BTreeMap<String, Vec<String>>,
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The icons that entry (its JSON) names: each one's name in nami's icons folder (`cask:<token>`)
/// and sha256. A name no cask could have, or a sha256 of another shape, is left out.
pub fn named_icons(entry: &[u8]) -> Vec<(String, String)> {
    let Ok(set) = serde_json::from_slice::<IconSet>(entry) else { return Vec::new() };
    set.cask_icons.into_iter().filter(|(t, sha)| valid_token(t) && is_sha256(sha)).map(|(t, sha)| (format!("{PREFIX}{t}"), sha)).collect()
}

/// The sha256s of a cask's screenshots that entry names (at most 8).
pub fn named_screenshots(entry: &[u8], token: &str) -> Vec<String> {
    let Ok(mut set) = serde_json::from_slice::<IconSet>(entry) else { return Vec::new() };
    set.cask_screenshots.remove(token).unwrap_or_default().into_iter().filter(|s| is_sha256(s)).take(8).collect()
}

/// A cask's screenshots, from the Mac App Store when it sells the app (the set's): where each
/// is, among the catalog's objects beside the catalog this Mac has. `nami info --screenshots`
/// fetches them (query::fetch_screenshots), each kept only if it's the sha256 the signed
/// catalog names.
pub fn screenshots(index: &crate::index::Index, token: &str) -> Vec<String> {
    let Some(entry) = index.raw(ICONS_ENTRY) else { return Vec::new() };
    let shas = named_screenshots(entry, token);
    if shas.is_empty() {
        return Vec::new();
    }
    let Some(objects) = crate::index::objects_url(&crate::migrate::var_dir(&root())) else { return Vec::new() };
    shas.into_iter().map(|s| format!("{objects}/{s}")).collect()
}

/// The sha256s of the icons the catalog has for `tokens` that come when they're shown.
pub fn named_later(entry: &[u8], tokens: &[&str]) -> BTreeMap<String, String> {
    let Ok(set) = serde_json::from_slice::<IconSet>(entry) else { return BTreeMap::new() };
    tokens.iter().filter(|t| valid_token(t)).filter_map(|t| set.cask_icons_later.get(*t).filter(|sha| is_sha256(sha)).map(|sha| (t.to_string(), sha.clone()))).collect()
}

/// The icons of `tokens` that don't come with `nami update`: each from the catalog's objects
/// (several at once), kept in the cache as `<sha256>.png` only if it's that sha256; one that's
/// there already isn't fetched again. Token -> its file, for those there are.
pub fn later_icons(index: &crate::index::Index, tokens: &[&str]) -> BTreeMap<String, PathBuf> {
    let Some(entry) = index.raw(ICONS_ENTRY) else { return BTreeMap::new() };
    let Some(dir) = cache_dir().map(|d| d.join("icons")) else { return BTreeMap::new() };
    later_icons_in(entry, tokens, || crate::index::objects_url(&crate::migrate::var_dir(&root())), &dir)
}

/// `later_icons`, with the catalog's entry, where its objects are, and the folder they're kept in.
fn later_icons_in(entry: &[u8], tokens: &[&str], objects: impl FnOnce() -> Option<String>, dir: &Path) -> BTreeMap<String, PathBuf> {
    let named = named_later(entry, tokens);
    let file = |sha: &str| dir.join(format!("{sha}.png"));
    let missing: Vec<&String> = named.values().filter(|sha| !file(sha).is_file()).collect();
    if !missing.is_empty() {
        if let Some(objects) = objects() {
            let _ = std::fs::create_dir_all(dir);
            fetch_objects(&objects, &missing, &file);
        }
    }
    named.into_iter().map(|(t, sha)| (t, file(&sha))).filter(|(_, f)| f.is_file()).collect()
}

/// Objects (`<objects>/<sha256>`), eight at a time, each written to `file(sha)` if it's those bytes.
fn fetch_objects(objects: &str, shas: &[&String], file: &(dyn Fn(&str) -> PathBuf + Sync)) {
    let agent = http();
    std::thread::scope(|s| {
        for chunk in shas.chunks(shas.len().div_ceil(8).max(1)) {
            let agent = &agent;
            s.spawn(move || {
                for sha in chunk {
                    let mut data = Vec::new();
                    let got = agent.get(&format!("{objects}/{sha}")).call().ok().and_then(|r| r.into_reader().take(1 << 20).read_to_end(&mut data).ok());
                    if got.is_some() && crate::catalog::sha256_hex(&data) == **sha {
                        let _ = crate::store::write_atomic(&file(sha), &data);
                    }
                }
            });
        }
    });
}

fn root() -> PathBuf {
    PathBuf::from(std::env::var_os("NAMI_ROOT").unwrap_or_else(|| "/".into()))
}

/// The icon a cask is shown with: the set's, else (installed) its app's own.
pub fn icon(token: &str, installed: Option<&Entry>) -> Option<PathBuf> {
    if !valid_token(token) {
        return None;
    }
    let set = crate::migrate::var_dir(&root()).join("icons").join(format!("{PREFIX}{token}.png"));
    if set.is_file() {
        return Some(set);
    }
    installed.and_then(app_icon)
}

/// An installed cask's app's own icon, as a PNG in the person's cache (root's: nami's folder): made
/// once (`sips`), and not tried again for an app that has none it can read.
#[cfg(target_os = "macos")]
fn app_icon(e: &Entry) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let token = entry_token(e)?;
    let root = root();
    let dir = cache_dir()?.join("icons");
    for p in &e.paths {
        let path = if root == Path::new("/") { PathBuf::from(p) } else { root.join(p.trim_start_matches('/')) };
        let Ok(list) = std::fs::read_dir(path.join("Applications")) else { continue };
        let mut apps: Vec<PathBuf> = list.flatten().map(|d| d.path()).filter(|a| a.extension().is_some_and(|x| x == "app")).collect();
        apps.sort();
        for app in apps {
            let Ok(app) = std::fs::canonicalize(&app) else { continue };
            let made = dir.join(format!("{token}-{}.png", &crate::catalog::sha256_hex(app.as_os_str().as_bytes())[..16]));
            if made.is_file() {
                return Some(made);
            }
            let none = made.with_extension("none");
            if none.exists() {
                continue;
            }
            if make_app_icon(&app, &made).is_some() {
                return Some(made);
            }
            let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&none, b""));
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn app_icon(_: &Entry) -> Option<PathBuf> {
    None
}

/// `app`'s icon as a PNG of at most 256 pixels, at `out`.
#[cfg(target_os = "macos")]
fn make_app_icon(app: &Path, out: &Path) -> Option<()> {
    let contents = app.join("Contents");
    let plist = Command::new("/usr/bin/plutil").args(["-convert", "xml1", "-o", "-"]).arg(contents.join("Info.plist")).stdin(Stdio::null()).stderr(Stdio::null()).output().ok().filter(|o| o.status.success())?;
    let xml = String::from_utf8_lossy(&plist.stdout);
    let resources: Vec<String> = std::fs::read_dir(contents.join("Resources")).ok()?.flatten().map(|d| d.file_name().to_string_lossy().into_owned()).collect();
    let file = app_icon_file(plist_string(&xml, "CFBundleIconFile").as_deref(), plist_string(&xml, "CFBundleIconName").as_deref(), &resources);
    std::fs::create_dir_all(out.parent()?).ok()?;
    let part = out.with_file_name(format!(".{}.{}.png", out.file_stem()?.to_string_lossy(), std::process::id()));
    let quiet = |c: &mut Command| c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    let made = match file {
        Some(file) => quiet(Command::new("/usr/bin/sips").args(["-s", "format", "png", "-Z", "256"]).arg(contents.join("Resources").join(&file)).arg("--out").arg(&part)),
        // Its icon only in Assets.car (no .icns): Quick Look's picture of the app, its icon.
        None => {
            let dir = out.with_file_name(format!(".{}.{}.ql", out.file_stem()?.to_string_lossy(), std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let ok = quiet(Command::new("/usr/bin/qlmanage").args(["-t", "-s", "256", "-o"]).arg(&dir).arg(app));
            let got = dir.join(format!("{}.png", app.file_name()?.to_string_lossy()));
            let ok = ok && std::fs::rename(&got, &part).is_ok();
            let _ = std::fs::remove_dir_all(&dir);
            ok
        }
    };
    if !made || !std::fs::read(&part).is_ok_and(|b| b.starts_with(b"\x89PNG\r\n\x1a\n")) {
        let _ = std::fs::remove_file(&part);
        return None;
    }
    std::fs::rename(&part, out).ok()
}

/// A string an XML plist has for `key` (`<key>K</key><string>V</string>`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn plist_string(xml: &str, key: &str) -> Option<String> {
    let at = xml.find(&format!("<key>{key}</key>"))? + key.len() + "<key></key>".len();
    let v = xml[at..].trim_start().strip_prefix("<string>")?;
    let v = &v[..v.find("</string>")?];
    let v = v.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Which of an app's Resources (`resources`, their names) is its icon: its Info.plist's
/// CFBundleIconFile (an .icns, or a PNG or TIFF by its whole name; `.icns` added when it has no
/// extension), else CFBundleIconName's .icns,
/// else AppIcon.icns. (packaging/cask-icons.py chooses as this does.)
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn app_icon_file(icon_file: Option<&str>, icon_name: Option<&str>, resources: &[String]) -> Option<String> {
    let has = |n: &str| resources.iter().find(|r| r.eq_ignore_ascii_case(n)).cloned();
    let named = |n: &str| {
        let n = n.trim().rsplit('/').next().unwrap_or_default();
        if n.is_empty() || n.starts_with('.') {
            return None;
        }
        has(n).filter(|f| [".icns", ".png", ".tiff", ".tif"].iter().any(|x| f.to_ascii_lowercase().ends_with(x))).or_else(|| has(&format!("{n}.icns")))
    };
    icon_file.and_then(named).or_else(|| icon_name.and_then(named)).or_else(|| has("AppIcon.icns"))
}

/// The Macs the set is made for: a cask is in it when nami installs it on one of them (Apple
/// silicon on macOS 26 or 15, else Intel on 15), its download and app as for the first.
const ICON_MACS: [Platform; 3] = [Platform { arm: true, macos: (26, 0) }, Platform { arm: true, macos: (15, 0) }, Platform { arm: false, macos: (15, 0) }];

/// The casks the set of icons is for (`nami catalog cask-icons`): the `top` most installed
/// (`installs`: Homebrew's counts) that nami installs (`plan`, on `ICON_MACS`), most installed
/// first, and then each of `also` (Discover's, in nami's app) that it installs too.
pub fn for_icons(all: &[Value], installs: &BTreeMap<String, u64>, top: usize, also: &[String]) -> Vec<(Plan, u64)> {
    let by: BTreeMap<&str, &Value> = all.iter().filter_map(|v| Some((v.get("token")?.as_str()?, v))).collect();
    let mut ranked: Vec<(&str, u64)> = by.keys().map(|t| (*t, installs.get(*t).copied().unwrap_or(0))).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let planned = |t: &str| -> Option<Plan> {
        let v = by.get(t)?;
        ICON_MACS.iter().find_map(|here| plan(&Cask::read(v, here).ok()?, here).ok())
    };
    let mut out: Vec<(Plan, u64)> = Vec::new();
    for (t, n) in ranked {
        if out.len() >= top {
            break;
        }
        if let Some(p) = planned(t) {
            out.push((p, n));
        }
    }
    for t in also {
        let t = token(t).unwrap_or(t);
        if out.iter().all(|(p, _)| p.token != t) {
            if let Some(p) = planned(t) {
                out.push((p, installs.get(t).copied().unwrap_or(0)));
            }
        }
    }
    out
}

/// What packaging/cask-icons.py needs of a cask in the set: its token, name and version, its
/// download (how it's asked for, its sha256, an archive inside it), and where its apps are in it.
pub fn icon_job(p: &Plan, installs: u64) -> Value {
    serde_json::json!({
        "token": p.token,
        "name": p.name,
        "version": p.version,
        "installs": installs,
        "url": p.url,
        "sha256": p.sha256,
        "headers": p.headers,
        "nested": p.nested,
        "apps": p.apps.iter().map(|a| a.source.clone()).collect::<Vec<_>>(),
    })
}

// ---------- Search, info and list ----------

/// A cask as nami lists packages (`nami search --json`, `nami info`). `installed`: the version
/// installed.
pub fn item(s: &Summary, installed: Option<&str>) -> crate::query::Item {
    let version = installed.unwrap_or(&s.version).to_string();
    let mut about = Vec::new();
    if s.auto_updates {
        about.push(format!("{} updates itself.", s.name));
    }
    crate::query::Item {
        attr: format!("{PREFIX}{}", s.token),
        name: entry_name(&s.token, &version),
        version,
        description: s.desc.clone(),
        homepage: s.homepage.clone(),
        license: None,
        app: true,
        installed: installed.is_some(),
        program: None,
        icon: None,
        unfree: false,
        builds_here: false,
        publisher: None,
        title: Some(s.name.clone()),
        summary: s.desc.clone(),
        about: (!about.is_empty()).then(|| about.join(" ")),
        categories: Vec::new(),
        screenshots: Vec::new(),
        popularity: None,
        desktop: Vec::new(),
        source: Some(SOURCE.to_string()),
        unavailable: s.refusal.clone(),
    }
}

/// How well a cask matches `q` (lowercase): 0 when it doesn't.
fn score(s: &Summary, q: &str) -> i32 {
    let (t, n) = (s.token.as_str(), s.name.to_lowercase());
    let starts = |text: &str| text.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q));
    let base = if t == q || n == q {
        100
    } else if t.starts_with(q) || starts(&n) {
        70
    } else if q.len() >= 4 && (t.contains(q) || n.contains(q)) {
        60
    } else if q.len() >= 3 && s.desc.as_deref().is_some_and(|d| starts(&d.to_lowercase())) {
        40
    } else {
        return 0;
    };
    // The most used, among those that match as well.
    base + ((s.installs.max(1) as f64).log10() * 4.0) as i32
}

/// The casks for `q` nami can install here, best first.
pub fn search_list<'a>(list: &'a [Summary], q: &str, limit: usize) -> Vec<&'a Summary> {
    let q = q.trim().to_lowercase();
    if q.chars().count() < 2 {
        return Vec::new();
    }
    let mut hits: Vec<(i32, &Summary)> = list.iter().filter(|s| s.refusal.is_none()).map(|s| (score(s, &q), s)).filter(|(n, _)| *n > 0).collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.installs.cmp(&a.1.installs)).then(a.1.token.cmp(&b.1.token)));
    hits.into_iter().take(limit).map(|(_, s)| s).collect()
}

/// The installed version of each cask in `installed` (entries), by token.
pub fn installed_versions(installed: &[Entry]) -> BTreeMap<String, String> {
    installed.iter().filter_map(|e| Some((entry_token(e)?.to_string(), entry_version(e)))).collect()
}

/// Homebrew's casks for a search, after the catalog's (`items`): a catalog package of the same
/// name that a Mac hasn't ready-built goes, as installing that name gives the cask.
/// (`everyone`: the computer's packages, whose installed apps have their icons too.)
pub fn add_to_search(items: &mut Vec<crate::query::Item>, q: &str, limit: usize, index: &crate::index::Index, installed: &[Entry], everyone: &[Entry]) {
    let Some(list) = list() else { return };
    let hits = search_list(list, q, limit);
    if hits.is_empty() {
        return;
    }
    items.retain(|i| !(hits.iter().any(|s| s.token == i.attr) && index.get(&i.attr).is_ok_and(|p| !ready_built(p))));
    let have = installed_versions(installed);
    let both: Vec<Entry> = installed.iter().chain(everyone).cloned().collect();
    // (The icons that come when they're shown, all at once first.)
    later_icons(index, &hits.iter().map(|s| s.token.as_str()).collect::<Vec<_>>());
    items.extend(hits.into_iter().map(|s| dressed(item(s, have.get(&s.token).map(String::as_str)), &s.token, &both, Some(index))));
}

/// `nami info cask:<token>` (`everyone`: as for a search).
pub fn info(token: &str, installed: &[Entry], everyone: &[Entry], index: Option<&crate::index::Index>) -> Option<crate::query::Item> {
    let s = find(token)?;
    let both: Vec<Entry> = installed.iter().chain(everyone).cloned().collect();
    Some(dressed(item(s, installed_versions(installed).get(token).map(String::as_str)), token, &both, index))
}

/// What `nami list` says of an installed one.
pub fn list_item(e: &Entry) -> Option<crate::query::Item> {
    let token = entry_token(e)?;
    let version = entry_version(e);
    let s = find(token).cloned().unwrap_or_else(|| Summary { token: token.to_string(), name: token.to_string(), ..Default::default() });
    let mut it = dressed(item(&s, Some(&version)), token, std::slice::from_ref(e), None);
    it.attr = e.attr.clone();
    Some(it)
}

/// An item with its app's icon, where there's one (`icon`; with the catalog, `index`, one that
/// comes when it's shown, `later_icons`), and its screenshots from the Mac App Store, where it
/// has them (`screenshots`).
fn dressed(mut it: crate::query::Item, token: &str, installed: &[Entry], index: Option<&crate::index::Index>) -> crate::query::Item {
    let entry = installed.iter().find(|e| entry_token(e) == Some(token));
    let set = crate::migrate::var_dir(&root()).join("icons").join(format!("{PREFIX}{token}.png"));
    let later = || index.and_then(|i| later_icons(i, &[token]).remove(token));
    let shown = if set.is_file() { Some(set) } else { later().or_else(|| icon(token, entry)) };
    it.icon = shown.map(|p| p.display().to_string());
    if let Some(index) = index {
        it.screenshots = screenshots(index, token);
    }
    it
}

/// The listed casks' newer versions: (attr, version now, Homebrew's), but not those that update
/// themselves.
pub fn updates(installed: &[Entry]) -> Vec<(String, String, String)> {
    updates_from(installed, find)
}

fn updates_from<'a>(installed: &[Entry], find: impl Fn(&str) -> Option<&'a Summary>) -> Vec<(String, String, String)> {
    installed.iter().filter_map(|e| {
        let s = find(entry_token(e)?)?;
        let now = entry_version(e);
        (kept(&now, Some(s), Upgrade::Newest, || false) == Keep::Newer).then(|| (e.attr.clone(), now, s.version.clone()))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disk_image_inside_a_zip_is_found() {
        let d = std::env::temp_dir().join(format!("nami-dmgzip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Switch/sub")).unwrap();
        std::fs::create_dir_all(d.join("__MACOSX")).unwrap();
        std::fs::create_dir_all(d.join("Other.app/Contents")).unwrap();
        std::fs::write(d.join("Switch/Switch.dmg"), b"x").unwrap();
        std::fs::write(d.join("__MACOSX/._Switch.dmg"), b"x").unwrap();
        std::fs::write(d.join("Other.app/Contents/x.dmg"), b"x").unwrap();
        std::fs::write(d.join("Switch/sub/deeper.dmg"), b"x").unwrap();
        assert_eq!(disk_images_in(&d), vec![d.join("Switch/Switch.dmg")]);
        std::fs::write(d.join("Top.DMG"), b"x").unwrap();
        assert_eq!(disk_images_in(&d).len(), 2);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn signatures_go_back_only_inside_the_app_and_by_their_names() {
        use base64::Engine;
        let d = std::env::temp_dir().join(format!("nami-sig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("VLC.app/Contents/MacOS/plugins")).unwrap();
        std::fs::write(d.join("VLC.app/Contents/MacOS/plugins/a.jar"), b"jar").unwrap();
        std::os::unix::fs::symlink("/etc", d.join("VLC.app/Contents/out")).unwrap();
        let v = base64::engine::general_purpose::STANDARD.encode(b"sig");
        let sig = |path: &str, name: &str| Signature { path: path.into(), name: name.into(), value: v.clone() };
        let mut total = 0;
        // A plug-in's CodeDirectory: where it is, as it is.
        let (at, name, value) = signature_place(&d, &sig("VLC.app/Contents/MacOS/plugins/a.jar", "com.apple.cs.CodeDirectory"), &mut total).unwrap();
        assert_eq!((at, name.as_str(), value.as_slice()), (d.join("VLC.app/Contents/MacOS/plugins/a.jar"), "com.apple.cs.CodeDirectory", &b"sig"[..]));
        // Not outside it, not through a link, not anything but a signature, not what isn't there.
        for (path, name) in [
            ("../x", "com.apple.cs.CodeDirectory"),
            ("/etc/passwd", "com.apple.cs.CodeDirectory"),
            ("VLC.app/Contents/out/hosts", "com.apple.cs.CodeDirectory"),
            ("VLC.app/Contents/out", "com.apple.cs.CodeDirectory"),
            ("VLC.app/Contents/MacOS/plugins/a.jar", "com.apple.quarantine"),
            ("VLC.app/Contents/MacOS/plugins/a.jar", "com.apple.ResourceFork"),
            ("VLC.app/none", "com.apple.cs.CodeDirectory"),
            ("", "com.apple.cs.CodeDirectory"),
        ] {
            assert!(signature_place(&d, &sig(path, name), &mut total).is_err(), "{path} {name}");
        }
        // Nor too big.
        let big = Signature { path: "VLC.app/Contents/MacOS/plugins/a.jar".into(), name: "com.apple.cs.CodeSignature".into(), value: base64::engine::general_purpose::STANDARD.encode(vec![0u8; SIGNATURE_MAX + 1]) };
        assert!(signature_place(&d, &big, &mut total).is_err());
        // The list goes as they're put back; a bad one stops it.
        std::fs::write(d.join(SIGNATURES), serde_json::to_vec(&vec![sig("VLC.app/Contents/MacOS/plugins/a.jar", "com.apple.cs.CodeDirectory")]).unwrap()).unwrap();
        put_signatures(&d).unwrap();
        assert!(!d.join(SIGNATURES).exists());
        std::fs::write(d.join(SIGNATURES), serde_json::to_vec(&vec![sig("../x", "com.apple.cs.CodeDirectory")]).unwrap()).unwrap();
        assert!(put_signatures(&d).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/casks").join(format!("{name}.json"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    const ARM_SEQUOIA: Platform = Platform { arm: true, macos: (15, 1) };
    const INTEL_SEQUOIA: Platform = Platform { arm: false, macos: (15, 3) };
    const ARM_TAHOE: Platform = Platform { arm: true, macos: (26, 0) };

    fn planned(name: &str, here: &Platform) -> std::result::Result<Plan, String> {
        plan(&Cask::read(&fixture(name), here).unwrap(), here)
    }

    #[test]
    fn tokens_and_entries() {
        assert!(valid_token("rectangle") && valid_token("google-chrome@beta") && valid_token("1password") && valid_token("visual-studio-code"));
        assert!(!valid_token("Rectangle") && !valid_token("-x") && !valid_token("a b") && !valid_token("../x") && !valid_token(""));
        assert_eq!(token("cask:rectangle"), Some("rectangle"));
        assert_eq!(token("rectangle"), None);
        let known = |t: &str| t == "google-chrome@beta";
        assert_eq!(split_version("rectangle@0.80", known), ("rectangle", Some("0.80")));
        assert_eq!(split_version("google-chrome@beta", known), ("google-chrome@beta", None));
        assert_eq!(split_version("rectangle", known), ("rectangle", None));
        let e = Entry { attr: "iterm2".into(), name: entry_name("iterm2", "3.7.9"), paths: vec!["/nix/store/x-cask-iterm2-3.7.9".into()] };
        assert!(is_cask_entry(&e));
        assert_eq!((entry_token(&e), entry_version(&e).as_str()), (Some("iterm2"), "3.7.9"));
        let e = Entry { attr: "cask:docker-desktop".into(), name: entry_name("docker-desktop", "4.99.0,245822"), paths: vec![] };
        assert_eq!((entry_token(&e), entry_version(&e).as_str()), (Some("docker-desktop"), "4.99.0,245822"));
        assert_eq!(store_name("docker-desktop", "4.99.0,245822"), "cask-docker-desktop-4.99.0_245822");
        assert!(crate::store::valid_store_name(&store_name("google-chrome@beta", "1 2")));
        assert!(!is_cask_entry(&Entry { attr: "ripgrep".into(), name: "ripgrep-15".into(), paths: vec![] }));
    }

    #[test]
    fn this_macs_names() {
        assert_eq!(ARM_SEQUOIA.tag().as_deref(), Some("arm64_sequoia"));
        assert_eq!(INTEL_SEQUOIA.tag().as_deref(), Some("sequoia"));
        assert_eq!(ARM_TAHOE.tag().as_deref(), Some("arm64_tahoe"));
        assert_eq!(Platform { arm: false, macos: (10, 15) }.tag().as_deref(), Some("catalina"));
        assert_eq!(Platform { arm: true, macos: (0, 0) }.tag(), None);
        assert_eq!(parse_macos("14"), Some((14, 0)));
        assert_eq!(parse_macos("10.15"), Some((10, 15)));
        assert_eq!(parse_macos("15.1.1"), Some((15, 1)));
        assert_eq!(parse_macos(":sonoma"), Some((14, 0)));
        assert_eq!(parse_macos("nonsense"), None);
        assert!(cmp_macos((15, 3), (15, 0)).is_eq());
        assert!(cmp_macos((10, 15), (11, 0)).is_lt());
    }

    #[test]
    fn an_app_alone() {
        let p = planned("rectangle", &ARM_SEQUOIA).unwrap();
        assert_eq!((p.token.as_str(), p.name.as_str(), p.version.as_str()), ("rectangle", "Rectangle", "2.0.5"));
        assert_eq!(p.url, "https://github.com/rxhanson/Rectangle/releases/download/v2.0.5/Rectangle2.0.5.dmg");
        assert_eq!(p.sha256.as_deref(), Some("4dec773a2f550d64147d88e07d987ce8119bdff045f1c6993a9a5eeb9f68496c"));
        assert_eq!(p.apps, vec![AppPart { source: "Rectangle.app".into(), target: "Rectangle.app".into() }]);
        assert!(p.links.is_empty() && p.headers.is_empty() && p.nested.is_none() && p.auto_updates);
        // macOS 14 or newer.
        let old = Platform { arm: true, macos: (13, 6) };
        assert_eq!(planned("rectangle", &old).unwrap_err(), "Rectangle 2.0.5 needs macOS 14 or newer; this Mac has macOS 13.6");
        let it = planned("iterm2", &INTEL_SEQUOIA).unwrap();
        assert_eq!((it.name.as_str(), it.apps[0].target.as_str()), ("iTerm2", "iTerm.app"));
    }

    #[test]
    fn each_mac_its_own_download() {
        // Raycast: Apple silicon's newest by default; Intel's own (an older version) as its variation says.
        let arm = planned("raycast", &ARM_SEQUOIA).unwrap();
        let intel = planned("raycast", &INTEL_SEQUOIA).unwrap();
        assert!(arm.url.contains("arm") && intel.url.contains("x86_64"), "{} {}", arm.url, intel.url);
        assert_ne!(arm.sha256, intel.sha256);
        assert_eq!(intel.version, "1.104.38");
        // On a macOS no variation names, the cask's own: there, it's for Apple silicon alone.
        let intel_tahoe = Platform { arm: false, macos: (26, 1) };
        assert_eq!(planned("raycast", &intel_tahoe).unwrap_err(), "Raycast is for Macs with Apple silicon; this one has an Intel processor");
        // Slack: Intel's download on Intel.
        assert!(planned("slack", &INTEL_SEQUOIA).unwrap().url.contains("/x64/"));
        assert!(planned("slack", &ARM_SEQUOIA).unwrap().url.contains("/arm64/"));
        // Itsycal: an older one where a newer macOS is needed for the newest.
        assert_eq!(planned("itsycal", &ARM_SEQUOIA).unwrap().version, "0.15.15");
    }

    #[test]
    fn no_checksum_and_programs_inside_the_app() {
        let chrome = planned("google-chrome", &ARM_SEQUOIA).unwrap();
        assert_eq!(chrome.sha256, None);
        assert_eq!(chrome.apps[0].target, "Google Chrome.app");
        // VS Code: `code` from inside its app, linked into the profile's bin.
        let code = planned("visual-studio-code", &ARM_SEQUOIA).unwrap();
        assert_eq!(code.links, vec![
            Link { name: "code".into(), app: "Visual Studio Code.app".into(), rest: "Contents/Resources/app/bin/code".into(), wrapper: false },
            Link { name: "code-tunnel".into(), app: "Visual Studio Code.app".into(), rest: "Contents/Resources/app/bin/code-tunnel".into(), wrapper: false },
        ]);
    }

    #[test]
    fn what_nami_doesnt_install_yet() {
        let zoom = planned("zoom", &ARM_SEQUOIA).unwrap_err();
        assert_eq!(zoom, "nami can't install Zoom yet: it needs Apple's installer (a .pkg), which runs with an administrator's rights and steps run as it's installed. For now, nami installs Homebrew's casks that are an app and nothing more.");
        let docker = planned("docker-desktop", &ARM_SEQUOIA).unwrap_err();
        assert_eq!(docker, format!("nami can't install Docker Desktop yet: it needs steps run as it's installed. {APP_ONLY}"));
        // A cask that needs another, or Homebrew's formulae; one that's disabled.
        let mut v = fixture("rectangle");
        v["depends_on"] = serde_json::json!({ "cask": ["some-helper"] });
        assert_eq!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap_err(), format!("Rectangle needs Homebrew's cask some-helper too, which nami doesn't install. {APP_ONLY}"));
        let mut v = fixture("rectangle");
        v["disabled"] = Value::Bool(true);
        v["disable_reason"] = Value::String("fails_gatekeeper_check".into());
        assert_eq!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap_err(), "Rectangle is disabled in Homebrew's casks (fails gatekeeper check), so nami doesn't install it");
        // A font; a git repo's; no checksum on http.
        let mut v = fixture("rectangle");
        v["artifacts"] = serde_json::json!([{ "font": ["X.ttf"] }]);
        assert!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap_err().contains("it needs fonts"));
        let mut v = fixture("rectangle");
        v["url"] = Value::String("git+https://example.org/x.git".into());
        assert!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap_err().contains("isn't a web address"));
        let mut v = fixture("google-chrome");
        v["url"] = Value::String("http://dl.google.com/chrome.dmg".into());
        assert!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap_err().contains("no checksum"));
        // An app somewhere it shouldn't be.
        let mut v = fixture("rectangle");
        v["artifacts"] = serde_json::json!([{ "app": ["../../Evil.app"] }]);
        assert!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).is_err());
        let mut v = fixture("rectangle");
        v["artifacts"] = serde_json::json!([{ "app": ["Rectangle.app", { "target": "/Applications/Utilities/Rect.app" }] }]);
        assert_eq!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap().apps[0].target, "Rect.app");
    }

    #[test]
    fn how_a_download_is_asked_for() {
        let mut v = fixture("rectangle");
        v["url_specs"] = serde_json::json!({ "user_agent": ":fake", "referer": "https://example.org/", "cookies": { "a": "1", "b": "2" }, "verified": "github.com" });
        let p = plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap();
        assert_eq!(p.headers, vec![("Cookie".to_string(), "a=1; b=2".to_string()), ("Referer".into(), "https://example.org/".into()), ("User-Agent".into(), BROWSER.into())]);
        v["url_specs"] = serde_json::json!({ "using": ":post", "data": { "x": "1" } });
        assert!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).is_err());
        let mut v = fixture("rectangle");
        v["container"] = serde_json::json!({ "nested": "Inner/Rectangle.dmg" });
        assert_eq!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap().nested.as_deref(), Some("Inner/Rectangle.dmg"));
        v["container"] = serde_json::json!({ "type": ":rar" });
        assert!(plan(&Cask::read(&v, &ARM_SEQUOIA).unwrap(), &ARM_SEQUOIA).unwrap_err().contains("(rar)"));
    }

    fn list() -> Vec<Summary> {
        let installs = read_installs(&serde_json::json!({ "items": [
            { "number": 1, "cask": "google-chrome", "count": "120,000" }, { "number": 2, "cask": "rectangle", "count": "15,751" },
            { "number": 3, "cask": "zoom", "count": "90,000" }, { "number": 4, "cask": "iterm2", "count": "30,000" },
        ]}));
        assert_eq!(installs.get("google-chrome"), Some(&120_000));
        ["rectangle", "google-chrome", "zoom", "raycast", "visual-studio-code", "docker-desktop", "itsycal", "slack", "iterm2"].iter().filter_map(|n| summarize(&fixture(n), &ARM_SEQUOIA, &installs)).collect()
    }

    #[test]
    fn the_list_and_a_search() {
        let list = list();
        assert_eq!(list.len(), 9);
        let zoom = list.iter().find(|s| s.token == "zoom").unwrap();
        assert!(zoom.refusal.as_deref().is_some_and(|r| r.contains(".pkg")) && zoom.apps.is_empty());
        let chrome = list.iter().find(|s| s.token == "google-chrome").unwrap();
        assert!(chrome.no_check && chrome.refusal.is_none() && chrome.apps == ["Google Chrome.app"] && chrome.installs == 120_000);
        // What nami can't install isn't found; the most used first among those as good.
        let names = |q: &str| search_list(&list, q, 10).iter().map(|s| s.token.clone()).collect::<Vec<_>>();
        assert_eq!(names("zoom"), Vec::<String>::new());
        assert_eq!(names("rect"), ["rectangle"]);
        assert_eq!(names("iterm"), ["iterm2"]);
        assert_eq!(names("chrome")[0], "google-chrome");
        assert_eq!(names("windows"), ["rectangle"], "{:?}", names("windows"));
        assert!(names("x").is_empty());
        // As nami lists packages.
        let it = item(list.iter().find(|s| s.token == "rectangle").unwrap(), Some("2.0.4"));
        assert_eq!((it.attr.as_str(), it.name.as_str(), it.version.as_str()), ("cask:rectangle", "cask:rectangle-2.0.4", "2.0.4"));
        assert!(it.installed && it.app && it.source.as_deref() == Some("cask") && it.about.as_deref() == Some("Rectangle updates itself."));
        assert_eq!(it.title.as_deref(), Some("Rectangle"));
        let z = item(zoom, None);
        assert!(z.unavailable.as_deref().is_some_and(|a| a.contains("Apple's installer")) && !z.installed);
    }

    #[test]
    fn the_icons_are_made_for_the_most_installed_that_nami_installs() {
        let all: Vec<Value> = ["rectangle", "google-chrome", "zoom", "raycast", "visual-studio-code", "docker-desktop", "itsycal", "slack", "iterm2"].iter().map(|n| fixture(n)).collect();
        let installs = read_installs(&serde_json::json!({ "items": [
            { "cask": "google-chrome", "count": "1,200,000" }, { "cask": "zoom", "count": "900,000" },
            { "cask": "iterm2", "count": "300,000" }, { "cask": "rectangle", "count": "157,510" },
        ]}));
        let tokens = |set: &[(Plan, u64)]| set.iter().map(|(p, _)| p.token.as_str()).collect::<Vec<_>>().join(" ");
        // Zoom (a .pkg) isn't one nami installs, so it has no icon made; the next one does.
        let set = for_icons(&all, &installs, 2, &[]);
        assert_eq!(tokens(&set), "google-chrome iterm2");
        assert_eq!(set[0].1, 1_200_000);
        // Discover's too, once each, and only those nami installs.
        let set = for_icons(&all, &installs, 2, &["cask:itsycal".into(), "cask:zoom".into(), "cask:google-chrome".into(), "no-such-cask".into()]);
        assert_eq!(tokens(&set), "google-chrome iterm2 itsycal");
        assert_eq!(for_icons(&all, &installs, 100, &[]).len(), 7, "{}", tokens(&for_icons(&all, &installs, 100, &[])));
        // What the script that makes them is told: Chrome's download has no checksum.
        let job = icon_job(&set[0].0, set[0].1);
        assert_eq!(job["token"], "google-chrome");
        assert_eq!(job["name"], "Google Chrome");
        assert_eq!(job["apps"], serde_json::json!(["Google Chrome.app"]));
        assert!(job["sha256"].is_null() && job["url"].as_str().unwrap().starts_with("https://") && job["installs"] == 1_200_000);
        let job = icon_job(&set[1].0, 0);
        assert_eq!(job["sha256"].as_str().map(str::len), Some(64));
    }

    #[test]
    fn the_catalog_names_the_icons() {
        let sha = "a".repeat(64);
        let entry = serde_json::json!({ "name": "nami-cask-icons", "outputs": {}, "uncached": true, "cask_icons": {
            "google-chrome": sha, "iterm2": "b".repeat(64), "../etc": sha, "Upper": sha, "short": "abc", "caps": "A".repeat(64),
        }});
        let named = named_icons(entry.to_string().as_bytes());
        assert_eq!(named, [("cask:google-chrome".to_string(), sha.clone()), ("cask:iterm2".to_string(), "b".repeat(64))]);
        // Any other shape names none.
        assert!(named_icons(b"{\"cask_icons\": [1]}").is_empty());
        assert!(named_icons(b"not json").is_empty());
        // And the Mac App Store's screenshots of an app it sells, by their sha256s.
        let mut entry = entry;
        entry["cask_screenshots"] = serde_json::json!({ "slack": ["c".repeat(64), "not one", "d".repeat(64)], "rectangle": [] });
        let e = entry.to_string();
        assert_eq!(named_screenshots(e.as_bytes(), "slack"), ["c".repeat(64), "d".repeat(64)]);
        assert!(named_screenshots(e.as_bytes(), "rectangle").is_empty() && named_screenshots(e.as_bytes(), "zoom").is_empty());
        assert_eq!(named_icons(e.as_bytes()).len(), 2);
        // The rest come as they're shown: by token, never with `nami update` (named_icons).
        entry["cask_icons_later"] = serde_json::json!({ "clocker": "e".repeat(64), "bad": "nope", "Upper": "e".repeat(64) });
        let e = entry.to_string();
        let later = named_later(e.as_bytes(), &["clocker", "bad", "Upper", "slack"]);
        assert_eq!(later.into_iter().collect::<Vec<_>>(), [("clocker".to_string(), "e".repeat(64))]);
        assert_eq!(named_icons(e.as_bytes()).len(), 2);
        // And it reads as a package to a nami from before it: no outputs, not ready-built.
        let pkg: crate::index::Package = serde_json::from_value(entry).unwrap();
        assert!(pkg.outputs.is_empty() && !ready_built(&pkg) && pkg.apps.is_none() && pkg.icon.is_none());
    }

    #[test]
    fn an_icon_comes_as_its_app_is_shown() {
        use std::io::{BufRead, BufReader};
        let (good, bad) = (b"\x89PNG\r\n\x1a\nclocker".to_vec(), b"\x89PNG\r\n\x1a\nkeka".to_vec());
        let (good_sha, bad_sha) = (crate::catalog::sha256_hex(&good), "f".repeat(64));
        // A server of objects: the right bytes for one, other bytes for the other.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (g, b, s, n) = (good.clone(), bad.clone(), good_sha.clone(), asked.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let mut r = BufReader::new(&stream);
                r.read_line(&mut line).unwrap();
                while { let mut h = String::new(); r.read_line(&mut h).unwrap() > 2 } {}
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = if line.contains(&format!("/objects/{s} ")) { &g } else { &b };
                let mut w = &stream;
                let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = w.write_all(body);
            }
        });
        let dir = std::env::temp_dir().join(format!("nami-later-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let entry = serde_json::json!({ "cask_icons_later": { "clocker": good_sha, "keka": bad_sha } }).to_string();
        let objects = || Some(format!("http://127.0.0.1:{port}/objects"));
        let got = later_icons_in(entry.as_bytes(), &["clocker", "keka", "slack"], objects, &dir);
        // Kept only as the catalog names it: keka's bytes aren't its sha256.
        assert_eq!(got.keys().collect::<Vec<_>>(), ["clocker"]);
        assert_eq!(std::fs::read(&got["clocker"]).unwrap(), good);
        assert_eq!(got["clocker"], dir.join(format!("{good_sha}.png")));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);
        // Once kept, not asked for again; without the network, what's kept.
        let got = later_icons_in(entry.as_bytes(), &["clocker"], || None, &dir);
        assert_eq!(got.len(), 1);
        later_icons_in(entry.as_bytes(), &["clocker"], objects, &dir);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_apps_icon_by_its_info_plist() {
        let xml = "<?xml version=\"1.0\"?><plist><dict>\n\t<key>CFBundleName</key>\n\t<string>Rectangle</string>\n\t<key>CFBundleIconFile</key>\n\t<string>AppIcon</string>\n\t<key>CFBundleIconName</key> <string>AppIcon &amp; more</string>\n</dict></plist>";
        assert_eq!(plist_string(xml, "CFBundleIconFile").as_deref(), Some("AppIcon"));
        assert_eq!(plist_string(xml, "CFBundleIconName").as_deref(), Some("AppIcon & more"));
        assert_eq!(plist_string(xml, "CFBundleName").as_deref(), Some("Rectangle"));
        assert_eq!(plist_string(xml, "CFBundleExecutable"), None);
        assert_eq!(plist_string("<key>CFBundleIconFile</key><true/>", "CFBundleIconFile"), None);
        let res = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let r = res(&["app.icns", "document.icns", "AppIcon.icns", "Assets.car"]);
        // CFBundleIconFile, with or without its extension; then CFBundleIconName; then AppIcon.
        assert_eq!(app_icon_file(Some("app"), None, &r).as_deref(), Some("app.icns"));
        assert_eq!(app_icon_file(Some("app.icns"), Some("x"), &r).as_deref(), Some("app.icns"));
        assert_eq!(app_icon_file(Some("App.ICNS"), None, &r).as_deref(), Some("app.icns"));
        assert_eq!(app_icon_file(Some("missing"), Some("document"), &r).as_deref(), Some("document.icns"));
        assert_eq!(app_icon_file(None, None, &r).as_deref(), Some("AppIcon.icns"));
        // Only an .icns, never a path out of Resources or another kind of file.
        assert_eq!(app_icon_file(Some("../../x"), None, &res(&["x.icns"])).as_deref(), Some("x.icns"));
        assert_eq!(app_icon_file(Some("Assets.car"), None, &res(&["Assets.car"])), None);
        // A PNG or TIFF it names as its icon, by its whole name (AltTab's app.png).
        assert_eq!(app_icon_file(Some("app.png"), None, &res(&["app.png", "pro-rabbit.png"])).as_deref(), Some("app.png"));
        assert_eq!(app_icon_file(Some("app"), None, &res(&["app.png"])), None);
        assert_eq!(app_icon_file(Some("."), None, &res(&[".icns"])), None);
        assert_eq!(app_icon_file(Some("icon"), Some("AppIcon"), &res(&["Assets.car"])), None);
    }

    #[test]
    fn which_names_are_casks() {
        let root = std::env::temp_dir().join(format!("nami-casks-want-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nix/var/nami")).unwrap();
        let out = |n: &str| format!("/nix/store/{}-{n}", "0".repeat(32));
        let index = serde_json::json!({
            "ripgrep": { "name": "ripgrep-15", "outputs": { "out": out("ripgrep-15") } },
            "iterm2": { "name": "iterm2-3.5", "outputs": { "out": out("iterm2-3.5") }, "uncached": true },
            "firefox": { "name": "firefox-150", "outputs": { "out": out("firefox-150") } },
        });
        std::fs::write(root.join("nix/var/nami/index.json"), index.to_string()).unwrap();
        let store = crate::store::Store::open_direct_readonly(&root).unwrap();
        let index = crate::index::Index::load(&store).unwrap();
        let casks = ["iterm2", "rectangle", "firefox", "google-chrome"];
        let packages: Vec<String> = ["ripgrep", "iterm2", "rectangle", "firefox", "cask:google-chrome", "nosuchthing", "rectangle@0.8", "./mine", "flathub:org.x.Y"].iter().map(|s| s.to_string()).collect();
        let got = want(&packages, Some(&index), |t| casks.contains(&t));
        assert_eq!(got, vec![
            Wanted { listed: "iterm2".into(), token: "iterm2".into(), why: Some("nixpkgs has no ready-built Mac build of it".into()) },
            Wanted { listed: "rectangle".into(), token: "rectangle".into(), why: Some("nixpkgs has no package of that name for a Mac".into()) },
            Wanted { listed: "cask:google-chrome".into(), token: "google-chrome".into(), why: None },
        ]);
        // No catalog: the plain names a cask has.
        assert_eq!(want(&packages, None, |t| casks.contains(&t)).len(), 4);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn gatekeepers_answers() {
        let app = Path::new("/x/Rectangle.app");
        assert_eq!(read_assessment(true, "/x/Rectangle.app: accepted\nsource=Notarized Developer ID\norigin=Developer ID Application: Ryan Hanson (XSYZ3E4B7D)\n", app), Some(Verdict::Notarized));
        assert_eq!(read_assessment(true, "/x/Rectangle.app: accepted\nsource=Developer ID\n", app), Some(Verdict::Accepted("Developer ID".into())));
        assert_eq!(read_assessment(false, "/x/Rectangle.app: rejected\nsource=Unnotarized Developer ID\n", app), Some(Verdict::Rejected("rejected".into())));
        assert_eq!(read_assessment(true, "/x/Rectangle.app: accepted\noverride=security disabled\n", app), None);
        let plist = "<dict>\n<key>mount-point</key>\n<string>/private/tmp/m/dmg.Ab12</string>\n</dict><dict><key>mount-point</key><string>/tmp/m/R &amp; D</string></dict>";
        assert_eq!(mount_points(plist), vec![PathBuf::from("/private/tmp/m/dmg.Ab12"), PathBuf::from("/tmp/m/R & D")]);
    }

    #[test]
    fn a_download_unpacked_and_staged() {
        let dir = std::env::temp_dir().join(format!("nami-casks-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A zip as a cask's: Visual Studio Code.app, with its `code`.
        let zip = dir.join("code.zip");
        {
            let mut z = Vec::new();
            let files: &[(&str, &[u8], u32)] = &[
                ("Visual Studio Code.app/Contents/Info.plist", b"<plist/>", 0o100644),
                ("Visual Studio Code.app/Contents/MacOS/Electron", b"electron", 0o100755),
                ("Visual Studio Code.app/Contents/Resources/app/bin/code", b"#!/bin/sh\n", 0o100755),
            ];
            let mut central = Vec::new();
            for (name, data, mode) in files {
                let at = z.len() as u32;
                let crc = crc32(data);
                z.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
                z.extend_from_slice(&crc.to_le_bytes());
                z.extend_from_slice(&(data.len() as u32).to_le_bytes());
                z.extend_from_slice(&(data.len() as u32).to_le_bytes());
                z.extend_from_slice(&(name.len() as u16).to_le_bytes());
                z.extend_from_slice(&0u16.to_le_bytes());
                z.extend_from_slice(name.as_bytes());
                z.extend_from_slice(data);
                central.extend_from_slice(b"PK\x01\x02\x14\x03\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
                central.extend_from_slice(&crc.to_le_bytes());
                central.extend_from_slice(&(data.len() as u32).to_le_bytes());
                central.extend_from_slice(&(data.len() as u32).to_le_bytes());
                central.extend_from_slice(&(name.len() as u16).to_le_bytes());
                central.extend_from_slice(&[0u8; 8]);
                central.extend_from_slice(&(mode << 16).to_le_bytes());
                central.extend_from_slice(&at.to_le_bytes());
                central.extend_from_slice(name.as_bytes());
            }
            let start = z.len() as u32;
            let size = central.len() as u32;
            z.extend_from_slice(&central);
            z.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
            z.extend_from_slice(&(files.len() as u16).to_le_bytes());
            z.extend_from_slice(&(files.len() as u16).to_le_bytes());
            z.extend_from_slice(&size.to_le_bytes());
            z.extend_from_slice(&start.to_le_bytes());
            z.extend_from_slice(&0u16.to_le_bytes());
            std::fs::write(&zip, z).unwrap();
        }
        assert_eq!(kind_of(&zip).unwrap(), Kind::Zip);
        std::fs::write(dir.join("x.pkg"), b"xar!\x00\x1c").unwrap();
        assert_eq!(kind_of(&dir.join("x.pkg")).unwrap(), Kind::Pkg);
        assert!(unpack(&dir.join("x.pkg"), &dir.join("p"), &dir).unwrap_err().to_string().contains(".pkg"));
        // (A disk image starts as its first block, compressed: bzip2's BZh in Chrome's, not a tar.)
        let mut dmg = b"BZh91AY&SY".to_vec();
        dmg.resize(2048, 0);
        dmg[2048 - 512..2048 - 508].copy_from_slice(b"koly");
        std::fs::write(dir.join("x.dmg"), dmg).unwrap();
        assert_eq!(kind_of(&dir.join("x.dmg")).unwrap(), Kind::Dmg);
        unpack(&zip, &dir.join("unpacked"), &dir).unwrap();
        let plan = planned("visual-studio-code", &ARM_SEQUOIA).unwrap();
        let folder = Path::new("/Users/ada/Applications/nami Apps");
        stage_apps(&plan, &dir.join("unpacked"), &dir.join("stage"), folder).unwrap();
        let stage = dir.join("stage");
        assert_eq!(std::fs::read_to_string(stage.join("Applications/Visual Studio Code.app/Contents/MacOS/Electron")).unwrap(), "electron");
        // `code` leads to the copy in nami Apps; code-tunnel isn't in this app, so there's none.
        assert_eq!(std::fs::read_link(stage.join("bin/code")).unwrap(), folder.join("Visual Studio Code.app/Contents/Resources/app/bin/code"));
        assert!(stage.join("bin/code-tunnel").symlink_metadata().is_err());
        // A wrapper: a script that starts the copy's program.
        let wrapped = Plan { links: vec![Link { name: "vsc".into(), app: "Visual Studio Code.app".into(), rest: "Contents/MacOS/Electron".into(), wrapper: true }], ..plan.clone() };
        let _ = std::fs::remove_dir_all(&dir.join("unpacked"));
        unpack(&zip, &dir.join("unpacked"), &dir).unwrap();
        stage_apps(&wrapped, &dir.join("unpacked"), &dir.join("stage2"), folder).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("stage2/bin/vsc")).unwrap(), "#!/bin/sh\nexec '/Users/ada/Applications/nami Apps/Visual Studio Code.app/Contents/MacOS/Electron' \"$@\"\n");
        // An app the download hasn't got.
        let missing = Plan { apps: vec![AppPart { source: "Other.app".into(), target: "Other.app".into() }], links: vec![], ..plan };
        assert!(stage_apps(&missing, &dir.join("unpacked"), &dir.join("stage3"), folder).unwrap_err().to_string().contains("has no Other.app"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A cask's apps kept in nami's casks folder (beside the store), the store path linking to
    /// them, nami Apps with a copy of them (a clone on a Mac), and gc taking a folder once no
    /// store path links to it (sparing a new one an install may not have linked yet).
    #[test]
    fn apps_kept_beside_the_store_and_gc() {
        let root = std::env::temp_dir().join(format!("nami-casks-kept-{}", std::process::id()));
        let _ = crate::store::remove_tree(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = crate::store::Store::open_direct(&root, false).unwrap();
        let work = root.join("work");
        let stage = work.join("stage");
        let exe = stage.join("Applications/Foo.app/Contents/MacOS/foo");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "foo 1").unwrap();
        let kept = keep_apps(&store, &stage.join("Applications"), "cask-foo-1.0", &work).unwrap();
        assert!(kept.starts_with(dir(&root)), "{}", kept.display());
        let name = kept.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("foo-1.0-") && name.len() == "foo-1.0-".len() + 12, "{name}");
        assert_eq!(std::fs::read_to_string(kept.join("Foo.app/Contents/MacOS/foo")).unwrap(), "foo 1");
        // The same again: the same folder.
        assert_eq!(keep_apps(&store, &stage.join("Applications"), "cask-foo-1.0", &work).unwrap(), kept);
        let plan = Plan { token: "foo".into(), name: "Foo".into(), version: "1.0".into(), url: String::new(), sha256: None, headers: vec![], nested: None, apps: vec![AppPart { source: "Foo.app".into(), target: "Foo.app".into() }], links: vec![], auto_updates: false, notes: vec![] };
        link_apps(&plan, &stage, &kept).unwrap();
        assert_eq!(std::fs::read_link(stage.join("Applications/Foo.app")).unwrap(), kept.join("Foo.app"));
        let path = store.add_path(&stage, "cask-foo-1.0").unwrap();
        // nami Apps: the app as it's kept (a copy where it can't be a clone), its folders writable.
        let apps = crate::mac_apps::of(&root, &[path.clone()]);
        assert_eq!(apps.len(), 1);
        let folder = root.join("home/Applications/nami Apps");
        crate::mac_apps::sync(&folder, &apps, &root, true).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("Foo.app/Contents/MacOS/foo")).unwrap(), "foo 1");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(folder.join("Foo.app/Contents")).unwrap().permissions().mode() & 0o700, 0o700);
        // Another version, kept, linked to by nothing: spared while it's new, then taken.
        let stage2 = work.join("stage2");
        std::fs::create_dir_all(stage2.join("Bar.app")).unwrap();
        std::fs::write(stage2.join("Bar.app/x"), "bar").unwrap();
        let orphan = keep_apps(&store, &stage2, "cask-bar-2.0", &work).unwrap();
        assert_eq!(sweep(&store), (0, 0));
        let age = |p: &Path| std::fs::File::open(p).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(2 * SPARE_FOR)).unwrap();
        age(&orphan);
        age(&kept);
        assert_eq!(sweep(&store), (1, 3));
        assert!(!orphan.exists() && kept.exists());
        // Once its store path goes (no step has it), its folder goes with it, in gc.
        store.gc(&[]).unwrap();
        assert!(!kept.exists());
        crate::store::remove_tree(&root).unwrap();
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn upgrades_leave_what_updates_itself() {
        let mut list = list();
        list.iter_mut().find(|s| s.token == "slack").unwrap().auto_updates = false;
        let find = |t: &str| list.iter().find(|s| s.token == t);
        let e = |attr: &str, token: &str, v: &str| Entry { attr: attr.into(), name: entry_name(token, v), paths: vec![] };
        // Slack, older and not updating itself; iTerm2, older but it updates itself; Rectangle, the newest.
        let mine = vec![e("cask:slack", "slack", "4.0"), e("iterm2", "iterm2", "3.6"), e("cask:rectangle", "rectangle", "2.0.5"), Entry { attr: "ripgrep".into(), name: "ripgrep-15".into(), paths: vec![] }];
        assert_eq!(updates_from(&mine, find), vec![("cask:slack".to_string(), "4.0".to_string(), "4.53.292".to_string())]);
        let closed = || false;
        assert_eq!(kept("3.6", find("iterm2"), Upgrade::Newest, closed), Keep::UpdatesItself);
        assert_eq!(kept("3.6", find("iterm2"), Upgrade::No, closed), Keep::As);
        assert_eq!(kept("4.0", find("slack"), Upgrade::Newest, closed), Keep::Newer);
        assert_eq!(kept("2.0.5", find("rectangle"), Upgrade::Newest, closed), Keep::As);
        // What nami can't install now stays as it was; one the list hasn't got too.
        assert_eq!(kept("7.0", find("zoom"), Upgrade::Newest, closed), Keep::As);
        assert_eq!(kept("1.0", None, Upgrade::Newest, closed), Keep::As);
        assert_eq!(installed_versions(&mine).get("iterm2").map(String::as_str), Some("3.6"));
    }

    /// The daily update leaves an app that's open as it is (the next day's has it); `nami
    /// upgrade`, asked for, doesn't. Whether it's open is only asked of an app that would move.
    #[test]
    fn the_daily_update_leaves_an_open_app() {
        let mut list = list();
        list.iter_mut().find(|s| s.token == "slack").unwrap().auto_updates = false;
        let find = |t: &str| list.iter().find(|s| s.token == t);
        let (closed, open) = (|| false, || true);
        let never = || -> bool { panic!("asked whether it's open") };
        assert_eq!(kept("4.0", find("slack"), Upgrade::NewestClosed, open), Keep::Open);
        assert_eq!(kept("4.0", find("slack"), Upgrade::NewestClosed, closed), Keep::Newer);
        assert_eq!(kept("4.0", find("slack"), Upgrade::Newest, never), Keep::Newer);
        // The newest already, one that updates itself, no upgrade at all: as they are, unasked.
        let newest = find("slack").unwrap().version.clone();
        assert_eq!(kept(&newest, find("slack"), Upgrade::NewestClosed, never), Keep::As);
        assert_eq!(kept("3.6", find("iterm2"), Upgrade::NewestClosed, never), Keep::UpdatesItself);
        assert_eq!(kept("4.0", find("slack"), Upgrade::No, never), Keep::As);
        // (An open app is still an update to show: Updates counts it.)
        let e = Entry { attr: "cask:slack".into(), name: entry_name("slack", "4.0"), paths: vec![] };
        assert_eq!(updates_from(&[e], find).len(), 1);
    }

    /// An app's open when a program from inside its copy in nami Apps runs (by the cask's apps'
    /// names); not when it's another app, or one of the same name elsewhere.
    #[test]
    fn an_app_is_open_by_its_copy_in_nami_apps() {
        let s = Summary { token: "slack".into(), apps: vec!["Slack.app".into()], ..Default::default() };
        // (macOS says where a program runs from without links: /private/var, not /var.)
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("nami-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let folder = d.join("nami Apps");
        assert!(!is_open(&s, &folder));
        if !crate::index::MAC {
            return;
        }
        let program = folder.join("Slack.app/Contents/MacOS/Slack");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::copy("/bin/sleep", &program).unwrap();
        std::fs::create_dir_all(d.join("Other/Slack.app/Contents/MacOS")).unwrap();
        let mut running = Command::new(&program).arg("30").spawn().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let elsewhere = Summary { apps: vec!["Slack.app".into()], ..Default::default() };
        let other = Summary { apps: vec!["Discord.app".into()], ..Default::default() };
        let (here, there, not) = (is_open(&s, &folder), is_open(&elsewhere, &d.join("Other")), is_open(&other, &folder));
        running.kill().unwrap();
        let _ = running.wait();
        std::fs::remove_dir_all(&d).unwrap();
        assert!(here, "Slack runs from nami Apps");
        assert!(!there && !not);
    }
}
