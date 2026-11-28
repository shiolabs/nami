//! How a git repo, and what's in it, is written: one form for a set from a
//! repo (a layer), a tool another project shares, and a config repo.
//!
//! `<repo>[/path][@ref][#name]`:
//!
//! - the repo is `github:owner/repo`, or any git URL after `git+`
//!   (`git+https://codeberg.org/bob/os.git`); `nami apply` also takes a URL
//!   alone (`https://…`, `git@host:cfg.git`).
//! - `/path` is a folder or file in the repo, after a GitHub repo only (a
//!   URL's own path can't be told from it): `github:alice/os/gaming.nami`.
//! - `@ref` is always the branch, tag or commit: `github:alice/os@v2`,
//!   `git+https://codeberg.org/bob/os.git@v2`.
//! - `#name` is always a name inside the repo: a set's file, a shared tool,
//!   or a config repo's computer (`git+https://…/os.git@v2#gaming.nami`,
//!   `github:acme/tools@v2#greet`, `git+https://…/cfg.git#laptop`).
//!
//! The earlier spellings still work until nami 1.0, with a notice that
//! shows the new one: a ref after the name (`git+https://…/os#gaming.nami@v2`,
//! the Installer's `git+https://…#laptop@main`) and, for a config repo, a URL's
//! `#ref` (`https://host/cfg.git#v2`).
//!
//! This file has no other part of nami in it: nami-gui reads addresses with it too.

use anyhow::{bail, ensure, Result};

/// The repo itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Repo {
    /// `github:owner/repo`.
    GitHub { owner: String, repo: String },
    /// A git URL, as git takes it.
    Url {
        url: String,
        /// Written with `git+` before it.
        plus: bool,
    },
}

/// A git reference as written: see the module's docs.
#[derive(Debug, Clone, PartialEq)]
pub struct GitRef {
    pub repo: Repo,
    /// A folder or file in the repo, after a GitHub repo ("": none).
    pub path: String,
    /// The branch, tag or commit (none: the repo's default branch).
    pub git_ref: Option<String>,
    /// A name inside the repo, after `#`.
    pub name: Option<String>,
    /// Written the earlier way: how it's written now.
    pub old: Option<String>,
}

/// What the reference is for: a config repo's URL alone took `#ref`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Use {
    /// A set from a repo, or a shared tool.
    Repo,
    /// A config repo (`nami apply`, the Installer's address).
    ConfigRepo,
}

pub const FORMS: &str = "github:owner/repo[/path][@ref][#name], or git+<url>[@ref][#name]";

pub fn name_ok(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('.') && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// A path in the repo: names, each a folder or file.
pub fn path_ok(p: &str) -> bool {
    p.is_empty() || p.split('/').all(name_ok)
}

fn ref_of(r: &str) -> Result<String> {
    ensure!(!r.is_empty(), "nothing after @: say a branch, tag or commit");
    ensure!(!r.starts_with('-') && !r.contains("..") && r.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c)), "{r:?} isn't a branch, tag or commit");
    Ok(r.to_string())
}

/// Is this a git repo's address (`github:`, `git+`, or a git URL alone)?
pub fn is_address(s: &str) -> bool {
    ["github:", "git+", "https://", "http://", "ssh://", "git://", "file://", "git@"].iter().any(|p| s.starts_with(p))
}

/// Where the path of a URL starts (after its host): an `@` from there on is a ref,
/// not the user before the host (`ssh://git@host/os.git@v2`, `git@host:os.git@v2`).
fn path_start(url: &str) -> usize {
    match url.find("://") {
        Some(i) => url[i + 3..].find('/').map_or(url.len(), |j| i + 3 + j),
        // scp-like: user@host:path
        None => url.find(':').map_or(0, |i| i + 1),
    }
}

pub fn parse(text: &str, used: Use) -> Result<GitRef> {
    let text = text.trim();
    let (before, frag) = match text.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (text, None),
    };
    let (repo, path, mut at) = if let Some(rest) = before.strip_prefix("github:") {
        let (p, r) = match rest.split_once('@') {
            Some((p, r)) => (p, Some(r)),
            None => (rest, None),
        };
        let mut parts = p.splitn(3, '/');
        let (owner, repo) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        ensure!(name_ok(owner) && name_ok(repo), "write it as {FORMS}");
        let path = parts.next().unwrap_or("").trim_matches('/').to_string();
        ensure!(path_ok(&path), "{path:?} isn't a path in the repo (like gaming.nami, or tools/cli)");
        (Repo::GitHub { owner: owner.into(), repo: repo.into() }, path, r.map(String::from))
    } else {
        let (url, plus) = match before.strip_prefix("git+") {
            Some(u) => (u, true),
            None => (before, false),
        };
        let scheme = ["https://", "http://", "ssh://", "git://", "file://"].iter().any(|s| url.starts_with(s)) && url.len() > 8;
        let scp = !plus && url.contains('@') && url.contains(':') && !url.contains("://");
        ensure!(scheme || scp, "{url:?}: a git URL (https://, ssh://, git:// or file://), as in {FORMS}");
        ensure!(!url.contains(char::is_whitespace), "{url:?}: a git URL has no spaces");
        let start = path_start(url);
        let (url, r) = match url[start..].rfind('@') {
            Some(i) => (&url[..start + i], Some(&url[start + i + 1..])),
            None => (url, None),
        };
        (Repo::Url { url: url.to_string(), plus }, String::new(), r.map(String::from))
    };
    let mut name = None;
    let mut old = false;
    if let Some(f) = frag {
        match f.split_once('@') {
            // The earlier way: the ref after the name (`#gaming.nami@v2`, `#@v2`).
            Some((n, r)) if at.is_none() => {
                at = Some(r.to_string());
                name = Some(n.to_string()).filter(|n| !n.is_empty());
                old = true;
            }
            Some(_) => bail!("{text}: one @ for the branch, tag or commit, before the #: {FORMS}"),
            // A config repo's URL alone took `#ref`.
            None if used == Use::ConfigRepo && matches!(repo, Repo::Url { plus: false, .. }) && at.is_none() => {
                at = Some(f.to_string());
                old = true;
            }
            None => name = Some(f.to_string()),
        }
    }
    if let Some(n) = &name {
        ensure!(path_ok(n.trim_matches('/')), "{n:?} isn't a name in the repo (like gaming.nami, greet or laptop)");
        name = Some(n.trim_matches('/').to_string());
    }
    let git_ref = at.as_deref().map(ref_of).transpose()?;
    let mut r = GitRef { repo, path, git_ref, name, old: None };
    if old {
        r.old = Some(r.to_string());
    }
    Ok(r)
}

impl GitRef {
    /// The URL git fetches it from.
    pub fn url(&self) -> String {
        match &self.repo {
            Repo::GitHub { owner, repo } => format!("https://github.com/{owner}/{repo}.git"),
            Repo::Url { url, .. } => url.clone(),
        }
    }

    /// The repo alone, as written (`github:alice/os`, `git+https://…`).
    pub fn repo_text(&self) -> String {
        match &self.repo {
            Repo::GitHub { owner, repo } => format!("github:{owner}/{repo}"),
            Repo::Url { url, plus: true } => format!("git+{url}"),
            Repo::Url { url, plus: false } => url.clone(),
        }
    }

    /// The repo and its ref (`github:alice/os@v2`): which commit, without what's in it.
    pub fn repo_at(&self) -> String {
        let at = self.git_ref.as_ref().map(|r| format!("@{r}")).unwrap_or_default();
        format!("{}{at}", self.repo_text())
    }

    /// The notice for an earlier spelling, if it is one.
    pub fn notice(&self, written: &str) -> Option<String> {
        let now = self.old.as_ref()?;
        Some(format!("{written} is written the earlier way: write {now} (the @ is the branch, tag or commit, the # a name in the repo). The earlier way works until nami 1.0."))
    }
}

impl std::fmt::Display for GitRef {
    /// How it's written now.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.repo_text())?;
        if !self.path.is_empty() {
            write!(f, "/{}", self.path)?;
        }
        if let Some(r) = &self.git_ref {
            write!(f, "@{r}")?;
        }
        if let Some(n) = &self.name {
            write!(f, "#{n}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> GitRef {
        parse(s, Use::Repo).unwrap()
    }

    #[test]
    fn the_new_form() {
        let r = p("github:alice/os/gaming.nami@v2");
        assert_eq!((r.path.as_str(), r.git_ref.as_deref(), r.name.as_deref(), r.old.as_deref()), ("gaming.nami", Some("v2"), None, None));
        assert_eq!(r.url(), "https://github.com/alice/os.git");
        assert_eq!(r.repo_at(), "github:alice/os@v2");
        let r = p("github:alice/os@v2#gaming.nami");
        assert_eq!((r.path.as_str(), r.git_ref.as_deref(), r.name.as_deref(), r.old), ("", Some("v2"), Some("gaming.nami"), None));
        let r = p("github:acme/tools/cli@v2#greet");
        assert_eq!((r.path.as_str(), r.git_ref.as_deref(), r.name.as_deref()), ("cli", Some("v2"), Some("greet")));
        let r = p("git+https://codeberg.org/bob/os.git@v2#gaming.nami");
        assert_eq!((r.url().as_str(), r.git_ref.as_deref(), r.name.as_deref(), r.old.as_deref()), ("https://codeberg.org/bob/os.git", Some("v2"), Some("gaming.nami"), None));
        assert_eq!(r.repo_at(), "git+https://codeberg.org/bob/os.git@v2");
        assert_eq!(r.to_string(), "git+https://codeberg.org/bob/os.git@v2#gaming.nami");
        // A ref with a slash; the user before a host isn't a ref.
        assert_eq!(p("github:alice/os@feature/x#gaming.nami").git_ref.as_deref(), Some("feature/x"));
        let r = p("git+ssh://git@host/os.git");
        assert_eq!((r.url().as_str(), r.git_ref), ("ssh://git@host/os.git", None));
        let r = p("git+ssh://git@host/os.git@main");
        assert_eq!((r.url().as_str(), r.git_ref.as_deref()), ("ssh://git@host/os.git", Some("main")));
        let r = parse("git@github.com:ada/cfg.git@v2", Use::ConfigRepo).unwrap();
        assert_eq!((r.url().as_str(), r.git_ref.as_deref(), r.old), ("git@github.com:ada/cfg.git", Some("v2"), None));
        let r = parse("git+https://example.org/ada/cfg.git@main#laptop", Use::ConfigRepo).unwrap();
        assert_eq!((r.git_ref.as_deref(), r.name.as_deref(), r.old), (Some("main"), Some("laptop"), None));
    }

    #[test]
    fn earlier_spellings_still_work_with_a_notice() {
        let r = p("git+https://codeberg.org/bob/os#gaming.nami@v2");
        assert_eq!((r.git_ref.as_deref(), r.name.as_deref()), (Some("v2"), Some("gaming.nami")));
        assert_eq!(r.old.as_deref(), Some("git+https://codeberg.org/bob/os@v2#gaming.nami"));
        let say = r.notice("git+https://codeberg.org/bob/os#gaming.nami@v2").unwrap();
        assert!(say.contains("write git+https://codeberg.org/bob/os@v2#gaming.nami") && say.contains("until nami 1.0"), "{say}");
        let r = p("git+file:///srv/os.git#@main");
        assert_eq!((r.git_ref.as_deref(), r.name, r.old.as_deref()), (Some("main"), None, Some("git+file:///srv/os.git@main")));
        // The Installer's: the computer, then the ref.
        let r = parse("git+https://x.org/os#laptop@main", Use::ConfigRepo).unwrap();
        assert_eq!((r.git_ref.as_deref(), r.name.as_deref(), r.old.as_deref()), (Some("main"), Some("laptop"), Some("git+https://x.org/os@main#laptop")));
        // A config repo's URL alone, with #ref.
        let r = parse("https://h.org/cfg.git#v2", Use::ConfigRepo).unwrap();
        assert_eq!((r.git_ref.as_deref(), r.name, r.old.as_deref()), (Some("v2"), None, Some("https://h.org/cfg.git@v2")));
        let r = parse("git@github.com:ada/cfg.git#v2", Use::ConfigRepo).unwrap();
        assert_eq!((r.url().as_str(), r.git_ref.as_deref(), r.old.as_deref()), ("git@github.com:ada/cfg.git", Some("v2"), Some("git@github.com:ada/cfg.git@v2")));
        // The new way says nothing.
        assert_eq!(p("github:alice/os/gaming.nami@v2").notice("x"), None);
    }

    #[test]
    fn refused() {
        for bad in [
            "github:alice",
            "github:../x",
            "github:alice/os/../etc",
            "github:alice/os@-x",
            "github:alice/os@",
            "git+/etc/os",
            "git+https://x/os#a@--upload-pack=x",
            "git+https://x/os@v1#a@v2",
            "gitlab:a/b",
            "git+https://x/a b",
            "github:alice/os#../etc",
        ] {
            assert!(parse(bad, Use::Repo).is_err(), "{bad}");
        }
    }
}
