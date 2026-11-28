//! Where a project's releases are. A repository on GitHub, GitLab or
//! Codeberg (and any Forgejo or Gitea) by its URL: its latest release, its
//! tags (read from git itself, `info/refs`, so no API and no limits), each
//! tag's source tarball, and a release's own tarball (with `configure`
//! made, for autotools projects) when it has one. Or a plain tarball URL:
//! its name and version from the file's name, and newer ones from the
//! listing of the folder it's in (ftp.gnu.org/gnu/hello/).

use std::cmp::Ordering;
use std::io::Read;

use anyhow::{anyhow, bail, Context, Result};

use super::version_cmp;
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forge {
    GitHub,
    GitLab,
    /// Codeberg, and any Forgejo or Gitea.
    Gitea,
}

/// A repository on a forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub forge: Forge,
    pub host: String,
    /// `owner/repo`, or GitLab's `group/subgroup/repo`.
    pub path: String,
}

/// What a URL names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream {
    /// A repository, at a tag if the URL names one (`/releases/tag/v1.2`),
    /// and the file if it's one of its tarballs or a release's file.
    Repo { repo: Repo, tag: Option<String>, file: Option<String> },
    /// Any other file.
    File(String),
}

impl Repo {
    pub fn name(&self) -> String {
        self.path.rsplit('/').next().unwrap_or(&self.path).to_lowercase()
    }

    pub fn url(&self) -> String {
        format!("https://{}/{}", self.host, self.path)
    }

    /// The source tarball the forge makes of a tag.
    pub fn tag_archive(&self, tag: &str) -> String {
        let t = url_encode(tag);
        match self.forge {
            Forge::GitHub => format!("https://github.com/{}/archive/refs/tags/{t}.tar.gz", self.path),
            Forge::GitLab => format!("https://{}/{}/-/archive/{t}/{}-{t}.tar.gz", self.host, self.path, self.path.rsplit('/').next().unwrap_or_default()),
            Forge::Gitea => format!("https://{}/{}/archive/{t}.tar.gz", self.host, self.path),
        }
    }

    /// Every tag, from git's own list of the repository's refs.
    pub fn tags(&self, store: &Store) -> Result<Vec<String>> {
        let url = format!("{}.git/info/refs?service=git-upload-pack", self.url());
        let resp = store.http().get(&url).call().with_context(|| format!("listing {}'s tags", self.url()))?;
        let mut data = Vec::new();
        resp.into_reader().take(64 << 20).read_to_end(&mut data)?;
        Ok(refs_tags(&data))
    }

    /// The tag of its latest release, if it has releases.
    pub fn latest_release(&self, store: &Store) -> Result<Option<String>> {
        match self.forge {
            // The page redirects to the release's own (no API, no limits).
            Forge::GitHub => {
                let resp = match store.http().get(&format!("{}/releases/latest", self.url())).call() {
                    Ok(r) => r,
                    Err(ureq::Error::Status(404, _)) => return Ok(None),
                    Err(e) => return Err(anyhow!("asking {} for its latest release: {e}", self.url())),
                };
                Ok(resp.get_url().split_once("/releases/tag/").map(|(_, t)| url_decode(t.split(['?', '#']).next().unwrap_or(t))))
            }
            Forge::GitLab => {
                let api = format!("https://{}/api/v4/projects/{}/releases/permalink/latest", self.host, url_encode(&self.path));
                Ok(self.json(store, &api)?.and_then(|v| v["tag_name"].as_str().map(String::from)))
            }
            Forge::Gitea => {
                let api = format!("https://{}/api/v1/repos/{}/releases/latest", self.host, self.path);
                Ok(self.json(store, &api)?.and_then(|v| v["tag_name"].as_str().map(String::from)))
            }
        }
    }

    fn json(&self, store: &Store, url: &str) -> Result<Option<serde_json::Value>> {
        match store.http().get(url).call() {
            Ok(r) => Ok(Some(serde_json::from_reader(r.into_reader().take(16 << 20)).with_context(|| format!("reading {url}"))?)),
            Err(ureq::Error::Status(404 | 403, _)) => Ok(None),
            Err(e) => Err(anyhow!("asking {url}: {e}")),
        }
    }

    /// The files of the release at `tag`, by URL (none if it isn't one).
    pub fn release_files(&self, store: &Store, tag: &str) -> Result<Vec<String>> {
        Ok(match self.forge {
            Forge::GitHub => {
                let page = format!("{}/releases/expanded_assets/{}", self.url(), url_encode(tag));
                let html = match store.http().get(&page).call() {
                    Ok(r) => r.into_string().unwrap_or_default(),
                    Err(_) => return Ok(Vec::new()),
                };
                let prefix = format!("/{}/releases/download/", self.path);
                hrefs(&html).into_iter().filter(|h| h.starts_with(&prefix)).map(|h| format!("https://github.com{h}")).collect()
            }
            Forge::GitLab => {
                let api = format!("https://{}/api/v4/projects/{}/releases/{}", self.host, url_encode(&self.path), url_encode(tag));
                let v = self.json(store, &api)?.unwrap_or_default();
                v["assets"]["links"].as_array().into_iter().flatten().filter_map(|l| l["url"].as_str().map(String::from)).collect()
            }
            Forge::Gitea => {
                let api = format!("https://{}/api/v1/repos/{}/releases/tags/{}", self.host, self.path, url_encode(tag));
                let v = self.json(store, &api)?.unwrap_or_default();
                v["assets"].as_array().into_iter().flatten().filter_map(|a| a["browser_download_url"].as_str().map(String::from)).collect()
            }
        })
    }

    /// The tag to build: the one asked for (by tag or version), else the
    /// latest release's, else the newest stable tag.
    pub fn pick_tag(&self, store: &Store, wanted: Option<&str>) -> Result<String> {
        let name = self.name();
        if let Some(w) = wanted {
            let tags = self.tags(store)?;
            return tags
                .iter()
                .find(|t| t.as_str() == w)
                .or_else(|| tags.iter().find(|t| tag_version(t, &name).as_deref() == Some(w.trim_start_matches('v'))))
                .cloned()
                .ok_or_else(|| anyhow!("{} has no tag for version {w}{}", self.url(), newest_hint(&tags, &name)));
        }
        if let Some(t) = self.latest_release(store)? {
            return Ok(t);
        }
        newest_tag(&self.tags(store)?, &name).ok_or_else(|| anyhow!("{} has no releases and no tags with a version: pass the tarball's URL, or --version <tag>", self.url()))
    }
}

fn newest_hint(tags: &[String], name: &str) -> String {
    match newest_tag(tags, name) {
        Some(t) => format!(" (its newest is {})", tag_version(&t, name).unwrap_or(t)),
        None => String::new(),
    }
}

/// The newest tag that's a stable version.
pub fn newest_tag(tags: &[String], name: &str) -> Option<String> {
    tags.iter()
        .filter_map(|t| Some((tag_version(t, name)?, t)))
        .filter(|(v, _)| is_stable(v))
        .max_by(|(a, _), (b, _)| version_cmp(a, b))
        .map(|(_, t)| t.clone())
}

/// The version a tag is: `v1.2.3`, `release-1.2.3`, `ripgrep-14.1.1`,
/// `curl-8_10_1` are 1.2.3 ... 8.10.1. None if it isn't one.
pub fn tag_version(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let mut v = lower.as_str();
    for prefix in [name.to_lowercase().as_str(), "release", "rel", "version", "ver", "stable"] {
        if let Some(r) = v.strip_prefix(prefix) {
            v = r.trim_start_matches(['-', '_', '/', '.']);
        }
    }
    let v = v.strip_prefix('v').unwrap_or(v);
    if !v.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let v = if !v.contains('.') && v.contains('_') { v.replace('_', ".") } else { v.to_string() };
    // A date is a version too (20240101), a commit isn't.
    (v.len() < 40).then_some(v)
}

/// A version without rc, alpha, beta, pre, dev... after its numbers.
pub fn is_stable(v: &str) -> bool {
    let rest = v.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').to_lowercase();
    !["rc", "alpha", "beta", "pre", "dev", "snapshot", "nightly", "preview", "test", "a", "b"].iter().any(|w| {
        let r = rest.trim_start_matches(['-', '_', '.', '+']);
        r.starts_with(w) && r[w.len()..].chars().next().is_none_or(|c| !c.is_ascii_alphabetic())
    })
}

/// The tag names in git's ref advertisement (`<sha> refs/tags/<tag>`).
fn refs_tags(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    let mut out: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.split('\0').next().unwrap_or(line);
        let Some(i) = line.find("refs/tags/") else { continue };
        let tag = line[i + "refs/tags/".len()..].trim().trim_end_matches("^{}");
        if !tag.is_empty() && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

/// What a URL names (see [`Upstream`]).
pub fn parse(url: &str) -> Upstream {
    let file = || Upstream::File(url.to_string());
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else { return file() };
    let rest = rest.split(['?', '#']).next().unwrap_or(rest).trim_end_matches('/');
    let Some((host, path)) = rest.split_once('/') else { return file() };
    let segs: Vec<&str> = path.split('/').collect();
    let is_archive = |f: &str| [".tar.gz", ".tgz", ".tar.xz", ".tar.bz2", ".tar.zst", ".zip"].iter().any(|e| f.ends_with(e));
    let strip_ext = |f: &str| {
        let mut f = f.to_string();
        for e in [".tar.gz", ".tgz", ".tar.xz", ".tar.bz2", ".tar.zst", ".zip"] {
            if let Some(s) = f.strip_suffix(e) {
                f = s.to_string();
                break;
            }
        }
        f
    };
    match host {
        "github.com" if segs.len() >= 2 => {
            let repo = Repo { forge: Forge::GitHub, host: host.into(), path: format!("{}/{}", segs[0], segs[1].trim_end_matches(".git")) };
            let (tag, file) = match &segs[2..] {
                [] | ["releases"] | ["tags"] | ["releases", "latest"] => (None, None),
                ["releases", "tag", t @ ..] | ["tree", t @ ..] => (Some(url_decode(&t.join("/"))), None),
                ["releases", "download", t, _asset] => (Some(url_decode(t)), Some(url.to_string())),
                ["archive", "refs", "tags", f] if is_archive(f) => (Some(url_decode(&strip_ext(f))), Some(url.to_string())),
                ["archive", f] if is_archive(f) => (Some(url_decode(&strip_ext(f))), Some(url.to_string())),
                _ => return file(),
            };
            Upstream::Repo { repo, tag, file }
        }
        "codeload.github.com" if segs.len() >= 4 => {
            let repo = Repo { forge: Forge::GitHub, host: "github.com".into(), path: format!("{}/{}", segs[0], segs[1]) };
            let tag = segs.last().map(|t| url_decode(t).trim_start_matches("refs/tags/").to_string());
            Upstream::Repo { repo, tag, file: Some(url.to_string()) }
        }
        _ if host == "gitlab.com" || host.starts_with("gitlab.") => {
            let (project, after) = match path.split_once("/-/") {
                Some((p, a)) => (p, a),
                None => (path, ""),
            };
            if !project.contains('/') {
                return file();
            }
            let repo = Repo { forge: Forge::GitLab, host: host.into(), path: project.trim_end_matches(".git").to_string() };
            let a: Vec<&str> = after.split('/').filter(|s| !s.is_empty()).collect();
            let (tag, file) = match a.as_slice() {
                [] | ["releases"] | ["tags"] => (None, None),
                ["releases", t] | ["tags", t] | ["tree", t] => (Some(url_decode(t)), None),
                ["archive", t, _f] => (Some(url_decode(t)), Some(url.to_string())),
                ["releases", t, "downloads", ..] => (Some(url_decode(t)), Some(url.to_string())),
                _ => return file(),
            };
            Upstream::Repo { repo, tag, file }
        }
        _ if (host == "codeberg.org" || host.starts_with("gitea.") || host.starts_with("forgejo.")) && segs.len() >= 2 => {
            let repo = Repo { forge: Forge::Gitea, host: host.into(), path: format!("{}/{}", segs[0], segs[1].trim_end_matches(".git")) };
            let (tag, file) = match &segs[2..] {
                [] | ["releases"] | ["tags"] => (None, None),
                ["releases", "tag", t] | ["src", "tag", t] => (Some(url_decode(t)), None),
                ["releases", "download", t, _] => (Some(url_decode(t)), Some(url.to_string())),
                ["archive", f] if is_archive(f) => (Some(url_decode(&strip_ext(f))), Some(url.to_string())),
                _ => return file(),
            };
            Upstream::Repo { repo, tag, file }
        }
        _ => file(),
    }
}

/// A tarball's name and version from its file's name: `hello-2.12.1.tar.gz`
/// is (hello, 2.12.1).
pub fn name_version(url: &str) -> Option<(String, String)> {
    let file = url.split(['?', '#']).next()?.rsplit('/').next()?;
    let stem = [".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst", ".tar", ".zip"].iter().find_map(|e| file.strip_suffix(e)).unwrap_or(file);
    // The version starts after the last `-` or `_` before a digit.
    let at = stem.char_indices().filter(|(i, c)| (*c == '-' || *c == '_') && stem[i + 1..].starts_with(|d: char| d.is_ascii_digit() || d == 'v')).map(|(i, _)| i).next()?;
    let (name, version) = (&stem[..at], stem[at + 1..].trim_start_matches('v'));
    (!name.is_empty() && version.starts_with(|d: char| d.is_ascii_digit())).then(|| (name.to_lowercase(), version.to_string()))
}

/// The newest version of a plain tarball, from the listing of the folder
/// it's in: files named like it with another version. (version, URL).
pub fn newest_in_folder(store: &Store, url: &str, version: &str) -> Result<Option<(String, String)>> {
    let (folder, file) = url.rsplit_once('/').ok_or_else(|| anyhow!("{url} isn't a file's URL"))?;
    let at = file.find(version).ok_or_else(|| anyhow!("{file} doesn't have its version ({version}) in its name, so nami can't tell which file is newer"))?;
    let (before, after) = (&file[..at], &file[at + version.len()..]);
    let listing = store.http().get(&format!("{folder}/")).call().with_context(|| format!("listing {folder}/"))?.into_string().unwrap_or_default();
    let mut best: Option<(String, String)> = None;
    for h in hrefs(&listing) {
        let name = h.rsplit('/').next().unwrap_or(&h);
        let Some(v) = name.strip_prefix(before).and_then(|r| r.strip_suffix(after)) else { continue };
        if v.is_empty() || !v.starts_with(|c: char| c.is_ascii_digit()) || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') || !is_stable(v) {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| version_cmp(v, b) == Ordering::Greater) {
            best = Some((v.to_string(), format!("{folder}/{name}")));
        }
    }
    Ok(best)
}

/// The release file that's its source with `configure` made, `<name>-<version>.tar.*`
/// (not a program built for a system).
pub fn source_file(files: &[String], name: &str, tag: &str, version: &str) -> Option<String> {
    let systems = ["linux", "darwin", "macos", "windows", "win32", "win64", "x86_64", "amd64", "aarch64", "arm64", "i686", "musl", "freebsd", "apple", ".deb", ".rpm", "-bin"];
    let mut found: Vec<&String> = files
        .iter()
        .filter(|u| {
            let f = crate::build::url_file_name(u).to_lowercase();
            let Some(stem) = [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"].iter().find_map(|e| f.strip_suffix(e)) else { return false };
            // <name>-<version>, the name its own or the repository's
            // with more to it (procps-ng-4.0.7 for procps).
            let prefix = [format!("-{version}"), format!("_{version}"), format!("-v{version}"), format!("-{}", tag.to_lowercase())].iter().find_map(|v| stem.strip_suffix(v.as_str())).map(String::from);
            prefix.is_some_and(|p| !p.is_empty() && (p.starts_with(name) || name.starts_with(&p))) && !systems.iter().any(|s| f.contains(s))
        })
        .collect();
    // xz first: smallest, and usually what's signed.
    found.sort_by_key(|u| if u.ends_with(".tar.xz") { 0 } else if u.ends_with(".tar.gz") { 1 } else { 2 });
    found.first().map(|u| u.to_string())
}

/// Every `href="..."` in a page.
fn hrefs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("href=") {
        rest = &rest[i + 5..];
        let q = rest.chars().next().unwrap_or(' ');
        let (start, end) = if q == '"' || q == '\'' { (1, rest[1..].find(q).map(|e| e + 1)) } else { (0, rest.find([' ', '>'])) };
        if let Some(end) = end {
            out.push(html_unescape(&rest[start..end]));
        }
    }
    out
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&#43;", "+").replace("&quot;", "\"")
}

pub fn url_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A URL with its version moved: the tag (`v1.2.0` to `v1.3.0`) and the
/// version (`1.2.0` to `1.3.0`) wherever they are in it.
pub fn moved_url(url: &str, old_tag: &str, new_tag: &str, old_version: &str, new_version: &str) -> Result<String> {
    let mut out = url.replace(old_tag, new_tag);
    if old_version != old_tag && !old_version.is_empty() {
        out = out.replace(old_version, new_version);
    }
    if out == url && old_tag != new_tag {
        bail!("{url} has neither the tag ({old_tag}) nor the version ({old_version}) in it, so nami can't tell the new one's URL: change source.url by hand");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_forge_urls() {
        let gh = |p: &str| Repo { forge: Forge::GitHub, host: "github.com".into(), path: p.into() };
        assert_eq!(parse("https://github.com/BurntSushi/ripgrep"), Upstream::Repo { repo: gh("BurntSushi/ripgrep"), tag: None, file: None });
        assert_eq!(parse("https://github.com/BurntSushi/ripgrep.git/"), Upstream::Repo { repo: gh("BurntSushi/ripgrep"), tag: None, file: None });
        assert_eq!(parse("https://github.com/junegunn/fzf/releases/tag/v0.56.3"), Upstream::Repo { repo: gh("junegunn/fzf"), tag: Some("v0.56.3".into()), file: None });
        let a = "https://github.com/sharkdp/hexyl/archive/refs/tags/v0.16.0.tar.gz";
        assert_eq!(parse(a), Upstream::Repo { repo: gh("sharkdp/hexyl"), tag: Some("v0.16.0".into()), file: Some(a.into()) });
        let d = "https://github.com/htop-dev/htop/releases/download/3.4.1/htop-3.4.1.tar.xz";
        assert_eq!(parse(d), Upstream::Repo { repo: gh("htop-dev/htop"), tag: Some("3.4.1".into()), file: Some(d.into()) });
        let Upstream::Repo { repo, tag, .. } = parse("https://gitlab.com/inkscape/lib2geom/-/releases/1.4") else { panic!() };
        assert_eq!((repo.forge, repo.path.as_str(), tag.as_deref()), (Forge::GitLab, "inkscape/lib2geom", Some("1.4")));
        assert_eq!(repo.tag_archive("1.4"), "https://gitlab.com/inkscape/lib2geom/-/archive/1.4/lib2geom-1.4.tar.gz");
        let Upstream::Repo { repo, tag, .. } = parse("https://codeberg.org/dnkl/foot/releases/tag/1.20.2") else { panic!() };
        assert_eq!((repo.forge, repo.name(), tag.as_deref()), (Forge::Gitea, "foot".to_string(), Some("1.20.2")));
        assert_eq!(repo.tag_archive("1.20.2"), "https://codeberg.org/dnkl/foot/archive/1.20.2.tar.gz");
        assert_eq!(gh("a/b").tag_archive("v1.0"), "https://github.com/a/b/archive/refs/tags/v1.0.tar.gz");
        assert_eq!(parse("https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz"), Upstream::File("https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz".into()));
        assert_eq!(name_version("https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz"), Some(("hello".into(), "2.12.1".into())));
        assert_eq!(name_version("https://x/foo-bar_v1.0.zip"), Some(("foo-bar".into(), "1.0".into())));
        assert_eq!(name_version("https://x/README"), None);
    }

    #[test]
    fn versions_from_tags() {
        assert_eq!(tag_version("v1.2.3", "x").as_deref(), Some("1.2.3"));
        assert_eq!(tag_version("ripgrep-14.1.1", "ripgrep").as_deref(), Some("14.1.1"));
        assert_eq!(tag_version("release-2.0", "x").as_deref(), Some("2.0"));
        assert_eq!(tag_version("curl-8_10_1", "curl").as_deref(), Some("8.10.1"));
        assert_eq!(tag_version("nightly", "x"), None);
        assert!(is_stable("1.2.3") && is_stable("2024.01.02") && !is_stable("1.2.3-rc1") && !is_stable("2.0.0b1") && !is_stable("1.0-beta"));
        let tags: Vec<String> = ["v0.9.0", "v0.10.0", "v0.11.0-rc1", "latest", "v0.10.1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(newest_tag(&tags, "x").as_deref(), Some("v0.10.1"));
        let refs = b"001e# service=git-upload-pack\n0000015b3a1f refs/heads/master\0multi_ack\n003f1a2b refs/tags/v1.0\n00423c4d refs/tags/v1.0^{}\n003f5e6f refs/tags/v1.1\n0000";
        assert_eq!(refs_tags(refs), ["v1.0", "v1.1"]);
        let sf = ["https://sourceforge.net/projects/procps-ng/files/Production/procps-ng-4.0.7.tar.xz/download".to_string()];
        assert_eq!(source_file(&sf, "procps", "v4.0.7", "4.0.7").as_deref(), Some(sf[0].as_str()));
        let files: Vec<String> = ["https://x/htop-3.4.1.tar.xz", "https://x/htop-3.4.1-linux-x86_64.tar.gz", "https://x/htop-3.4.1.tar.gz"].iter().map(|s| s.to_string()).collect();
        assert_eq!(source_file(&files, "htop", "3.4.1", "3.4.1").as_deref(), Some("https://x/htop-3.4.1.tar.xz"));
        assert_eq!(source_file(&files[1..2], "htop", "3.4.1", "3.4.1"), None);
        assert_eq!(moved_url("https://github.com/a/b/archive/refs/tags/v1.2.0.tar.gz", "v1.2.0", "v1.3.0", "1.2.0", "1.3.0").unwrap(), "https://github.com/a/b/archive/refs/tags/v1.3.0.tar.gz");
        assert_eq!(moved_url("https://x/r/download/3.4.1/htop-3.4.1.tar.xz", "3.4.1", "3.5.0", "3.4.1", "3.5.0").unwrap(), "https://x/r/download/3.5.0/htop-3.5.0.tar.xz");
        assert_eq!(hrefs("<a href=\"hello-2.12.tar.gz\">x</a> <a href='hello-2.12.1.tar.gz'>"), ["hello-2.12.tar.gz", "hello-2.12.1.tar.gz"]);
        assert_eq!(url_decode("v1.0%2Bbuild"), "v1.0+build");
    }
}
