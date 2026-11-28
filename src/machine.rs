//! `nami apply <repo>`: a computer set up from a config repo in one step.
//!
//! The repo holds a folder per computer, named after it (`laptop/`): its
//! system.nami and each person's file on it, <login>/user.nami (with its
//! nami.lock). What several computers share is in shared/, as the same two
//! files (listed by each: `- ../../shared/user.nami`), next to .nami-recipients.
//! It's cloned into ~/.config/nami and ~/user.nami links to your user.nami there, so `nami add` and your edits land in
//! the checkout, ready to commit and push; applying it again pulls first.
//! Git comes from the catalog when the computer has none.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};

use crate::index::Index;
use crate::store::{self, Store};

pub struct Flags {
    pub locked: bool,
    pub upgrade: bool,
    pub dry_run: bool,
}

/// Is this `nami apply` argument a repo rather than a file?
pub fn is_repo(arg: &str) -> bool {
    crate::gitref::is_address(arg)
}

/// `github:owner/repo/laptop[@ref]`, `<repo>[@ref]#laptop`: the repo (with
/// its ref), and which of its computers (its folder `laptop/`). One form
/// with layers and shared tools (gitref.rs); the earlier `#ref` of a URL,
/// and the Installer's `git+<url>#laptop@ref`, still work, with a notice.
pub fn split_computer(arg: &str) -> (String, Option<String>) {
    let Ok(mut r) = crate::gitref::parse(arg, crate::gitref::Use::ConfigRepo) else { return (arg.to_string(), None) };
    crate::layers::say_old(&r, arg);
    let mut computer = r.name.take();
    if computer.is_none() && !r.path.is_empty() {
        if r.path.contains('/') || crate::oldnames::ends_with_extension(&r.path) {
            return (arg.to_string(), None);
        }
        computer = Some(std::mem::take(&mut r.path));
    }
    (r.repo_at(), computer)
}

#[derive(Debug, PartialEq)]
struct Source {
    url: String,
    git_ref: Option<String>,
}

/// `github:owner/repo[@ref]`, or any git URL with an optional `@ref`
/// (`git+` before it, or not); the earlier `#ref` of a URL too, with a notice.
fn parse(arg: &str) -> Result<Source> {
    let forms = "write it as github:owner/repo[/<computer>][@branch], or git+<url>[@branch][#<computer>] (a folder per computer at its top)";
    let r = crate::gitref::parse(arg, crate::gitref::Use::ConfigRepo).map_err(|e| anyhow::anyhow!("{e}: {forms}"))?;
    crate::layers::say_old(&r, arg);
    ensure!(r.path.is_empty() && r.name.is_none(), "{forms}");
    Ok(Source { url: r.url(), git_ref: r.git_ref })
}

/// The same repo, however it's written: `git@github.com:o/r.git` and
/// `https://github.com/o/r` are one.
fn same_repo(a: &str, b: &str) -> bool {
    fn key(u: &str) -> String {
        let mut s = u.trim().trim_end_matches('/').trim_end_matches(".git").to_string();
        for scheme in ["https://", "http://", "ssh://", "git://"] {
            if let Some(r) = s.strip_prefix(scheme) {
                s = r.to_string();
            }
        }
        if let Some((_, rest)) = s.split_once('@').filter(|(user, _)| !user.contains('/')) {
            s = rest.to_string();
        }
        // scp-like host:path
        if let Some((host, path)) = s.split_once(':').filter(|(h, p)| !h.contains('/') && !p.starts_with("//")) {
            s = format!("{host}/{}", path.trim_start_matches('/'));
        }
        s.to_lowercase()
    }
    key(a) == key(b)
}

/// What nami writes into ~/.config/nami for this computer alone; never part
/// of the repo (the secrets key and decrypted secrets above all).
const OWN_FILES: &[&str] = &["/age.key", "/secrets.env", "/secrets.fish", "/env.sh", "/shell.*"];

/// ~/user.nami, the checkout's: what `nami add` edits is then the repo's.
/// A file of your own already there stays (nami says so).
fn link_user_file(user: &Path) -> Result<()> {
    let home = crate::config::user_file();
    match std::fs::symlink_metadata(&home) {
        Ok(m) if !m.file_type().is_symlink() => {
            eprintln!("{} is a file of your own, so it isn't the repo's: the repo's is {}", home.display(), user.display());
            Ok(())
        }
        Ok(_) if std::fs::read_link(&home).is_ok_and(|t| t == user) => Ok(()),
        found => {
            if found.is_ok() {
                std::fs::remove_file(&home)?;
            }
            std::os::unix::fs::symlink(user, &home).with_context(|| format!("linking {} to {}", home.display(), user.display()))
        }
    }
}

/// Keep this computer's own files in `dir` out of git: in a checkout's
/// .git/info/exclude, else a .gitignore (if there's none yet).
pub fn ignore_own_files(dir: &Path) {
    let (file, head) = if dir.join(".git").is_dir() {
        (dir.join(".git/info/exclude"), "")
    } else if !dir.join(".gitignore").exists() {
        (dir.join(".gitignore"), "# This computer's own files, written by nami: not for the repo.\n")
    } else {
        return;
    };
    let had = std::fs::read_to_string(&file).unwrap_or_default();
    let missing: Vec<&str> = OWN_FILES.iter().copied().filter(|l| !had.lines().any(|h| h.trim() == *l)).collect();
    if missing.is_empty() {
        return;
    }
    let mut text = if had.is_empty() { head.to_string() } else { had.clone() };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !had.is_empty() && head.is_empty() {
        text.push_str("# nami: this computer's own files\n");
    }
    for l in missing {
        text += l;
        text.push('\n');
    }
    let _ = std::fs::create_dir_all(file.parent().unwrap());
    let _ = std::fs::write(&file, text);
}

pub(crate) struct Git {
    bin: PathBuf,
    from_store: bool,
}

impl Git {
    /// The computer's git, or the catalog's (fetched once, through the daemon).
    pub(crate) fn get(root: &Path) -> Result<Git> {
        if let Some(bin) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("git")).find(|p| p.is_file()) {
            return Ok(Git { bin, from_store: false });
        }
        let store = Store::open(root)?;
        let index = Index::load(&store)?;
        let out = index.get("git")?.outputs.get("out").cloned().flatten().context("the catalog's git has no output")?;
        if !store.is_valid(store::base_name(&out)) {
            crate::say!("this computer has no git: fetching it (once)");
            store.realise(std::slice::from_ref(&out))?;
        }
        Ok(Git { bin: store.phys(&out).join("bin/git"), from_store: true })
    }

    /// The git at `bin` (one on PATH).
    #[cfg(test)]
    pub(crate) fn at(bin: PathBuf) -> Git {
        Git { bin, from_store: false }
    }

    pub(crate) fn cmd(&self, dir: Option<&Path>) -> Command {
        let mut c = Command::new(&self.bin);
        if let Some(d) = dir {
            c.arg("-C").arg(d);
        }
        if self.from_store {
            // Its helpers (git-remote-https...) next to it, wherever the store is.
            if let Some(pkg) = self.bin.parent().and_then(Path::parent) {
                c.env("GIT_EXEC_PATH", pkg.join("libexec/git-core"));
                c.env("GIT_TEMPLATE_DIR", pkg.join("share/git-core/templates"));
            }
            // It looks for certificates where NixOS keeps them.
            if std::env::var_os("GIT_SSL_CAINFO").is_none() {
                if let Some(f) = crate::runtime::ca_bundle() {
                    c.env("GIT_SSL_CAINFO", f);
                }
            }
        }
        c
    }

    fn run(&self, dir: Option<&Path>, args: &[&str]) -> Result<()> {
        let status = self.cmd(dir).args(args).status().with_context(|| format!("running {}", self.bin.display()))?;
        ensure!(status.success(), "git {} failed", args.first().unwrap_or(&""));
        Ok(())
    }

    fn output(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let o = self.cmd(Some(dir)).args(args).stderr(std::process::Stdio::null()).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }
}

/// Make `dir` a checkout of `src`: pull if it already is one, else clone
/// (whatever was there is moved aside first, when `aside` is allowed).
fn sync(git: &Git, src: &Source, dir: &Path, aside: bool) -> Result<()> {
    if dir.join(".git").exists() && git.output(dir, &["remote", "get-url", "origin"]).is_some_and(|o| same_repo(&o, &src.url)) {
        eprintln!("updating {} from {}", dir.display(), src.url);
        let updated = (|| -> Result<()> {
            git.run(Some(dir), &["fetch", "--quiet", "--tags", "origin"])?;
            if let Some(r) = &src.git_ref {
                git.run(Some(dir), &["checkout", "--quiet", r])?;
            }
            // nami.lock changes as packages are resolved here: the repo's
            // wins (apply resolves what's still new), anything else of yours
            // is kept or git says why not.
            // Layers pinned here that the repo's lock doesn't pin yet stay
            // pinned, so they aren't looked up (and moved) again each time.
            let mut kept = Vec::new();
            let changed = git.output(dir, &["status", "--porcelain", "--untracked-files=all"]).unwrap_or_default();
            for line in changed.lines() {
                let path = line.get(3..).unwrap_or("");
                if path == "nami.lock" || path.ends_with("/nami.lock") && path.matches('/').count() == 1 {
                    if let Ok(l) = crate::lock::load(&dir.join(path)) {
                        kept.push((dir.join(path), l.layers));
                    }
                    if line.starts_with("??") {
                        let _ = std::fs::remove_file(dir.join(path));
                    } else {
                        git.run(Some(dir), &["checkout", "--quiet", "--", path])?;
                    }
                }
            }
            if git.output(dir, &["symbolic-ref", "-q", "HEAD"]).is_some() {
                git.run(Some(dir), &["merge", "--quiet", "--ff-only", "@{upstream}"])?;
            }
            for (path, layers) in kept.into_iter().filter(|(_, l)| !l.is_empty()) {
                let mut lock = crate::lock::load(&path)?;
                let before = lock.layers.len();
                for (k, pin) in layers {
                    lock.layers.entry(k).or_insert(pin);
                }
                if lock.layers.len() != before {
                    crate::lock::save(&path, &lock)?;
                }
            }
            Ok(())
        })();
        if let Err(e) = updated {
            bail!("{e}: couldn't update {} from the repo (git said why, above). Sort it out there (`git status`), then run this again", dir.display());
        }
        return Ok(());
    }
    let mut moved = None;
    if dir.read_dir().is_ok_and(|mut d| d.next().is_some()) {
        ensure!(aside, "{} is in the way", dir.display());
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let to = dir.with_file_name(format!("{}.before-{secs}", dir.file_name().unwrap().to_string_lossy()));
        std::fs::rename(dir, &to).with_context(|| format!("moving {} aside", dir.display()))?;
        eprintln!("moved what was in {} to {}", dir.display(), to.display());
        moved = Some(to);
    }
    if let Some(p) = dir.parent() {
        std::fs::create_dir_all(p)?;
    }
    let _ = std::fs::remove_dir(dir); // an empty one
    git.run(None, &["clone", "--quiet", &src.url, &dir.display().to_string()]).with_context(|| format!("cloning {}", src.url))?;
    if let Some(r) = &src.git_ref {
        git.run(Some(dir), &["checkout", "--quiet", r])?;
    }
    // This computer's secrets key stays this computer's.
    if let Some(old) = moved {
        if old.join("age.key").is_file() && !dir.join("age.key").exists() {
            std::fs::rename(old.join("age.key"), dir.join("age.key"))?;
        }
    }
    Ok(())
}

/// A copy of the repo to read (cloned, or updated, under ~/.cache/nami/repos).
pub fn checkout(root: &Path, arg: &str) -> Result<PathBuf> {
    let src = parse(arg)?;
    let dir = scratch_dir(&src);
    let git = Git::get(root)?;
    if dir.exists() && !dir.join(".git").exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    sync(&git, &src, &dir, false)?;
    Ok(dir)
}

/// After `nami usb <repo>` let a computer open the repo's secrets: the
/// change is in nami's own checkout of it (`dir`), and has to reach the
/// repo, or the computer can't open them once it applies the repo itself.
/// Commit and push it if the person says so; else say exactly how. Also
/// when an earlier run's change is still there (not committed, or not
/// pushed). `files`: what this run changed. `answer`: whether to, without
/// asking (None: ask, on a terminal).
pub fn share_change(root: &Path, arg: &str, dir: &Path, files: &[PathBuf], host: &str, public: &str, answer: Option<bool>) -> Result<Shared> {
    let git = Git::get(root)?;
    let rel: Vec<String> = files.iter().map(|f| f.strip_prefix(dir).unwrap_or(f).display().to_string()).collect();
    // Changed or new, not committed (paths from the top).
    let uncommitted: Vec<String> = git.output(dir, &["ls-files", "--modified", "--others", "--exclude-standard", "--full-name"]).unwrap_or_default().lines().map(String::from).filter(|p| p == crate::secrets::RECIPIENTS || p.ends_with(".nami")).collect();
    let unpushed = git.output(dir, &["rev-list", "--count", "@{upstream}..HEAD"]).and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
    let mut to_commit: Vec<String> = rel.into_iter().chain(uncommitted).collect();
    to_commit.sort();
    to_commit.dedup();
    if to_commit.is_empty() && unpushed == 0 {
        return Ok(Shared::Nothing);
    }
    let message = format!("Let {host} open the secrets");
    let quoted: Vec<String> = to_commit.iter().map(|f| format!("'{}'", f.replace('\'', "'\\''"))).collect();
    let d = dir.display();
    eprintln!();
    if to_commit.is_empty() {
        eprintln!("{host} can open the secrets in nami's copy of {arg} ({d}), committed there and not pushed yet.");
    } else {
        eprintln!("{host} can open the secrets now in nami's copy of {arg} ({d}): {} changed.", to_commit.join(", "));
    }
    eprintln!("The USB drive has that already; the repo needs it too, or {host} can't open them when it applies {arg} itself.");
    let yes = answer.unwrap_or_else(|| crate::errors::may_ask()) && (answer.is_some() || {
        eprint!("Commit and push it to {arg} now? [y/N] ");
        std::io::Write::flush(&mut std::io::stderr())?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        matches!(line.trim(), "y" | "Y" | "yes")
    });
    let mut committed = to_commit.is_empty();
    if yes {
        let done = (|| -> Result<()> {
            if !committed {
                let mut args = vec!["commit", "--quiet", "-m", message.as_str(), "--"];
                args.extend(to_commit.iter().map(String::as_str));
                git.run(Some(dir), &["add", "--"].iter().copied().chain(to_commit.iter().map(String::as_str)).collect::<Vec<_>>())?;
                git.run(Some(dir), &args)?;
                committed = true;
            }
            git.run(Some(dir), &["push", "--quiet"])
        })();
        match done {
            Ok(()) => {
                eprintln!("pushed: {host} can open the secrets from {arg} now");
                return Ok(Shared::Pushed);
            }
            Err(e) => eprintln!("that didn't work ({e}; git said why, above). By hand, then:"),
        }
    } else {
        eprintln!("To do it by hand:");
    }
    if !committed {
        eprintln!("  git -C {d} add -- {}", quoted.join(" "));
        eprintln!("  git -C {d} commit -m '{message}' -- {}", quoted.join(" "));
    }
    eprintln!("  git -C {d} push");
    eprintln!("or, in your own checkout of the repo: `nami secret allow {host} {public}`, then commit and push.");
    Ok(Shared::Told)
}

/// What [`share_change`] did.
#[derive(Debug, PartialEq)]
pub enum Shared {
    /// Nothing to share.
    Nothing,
    Pushed,
    /// Said how.
    Told,
}

fn scratch_dir(src: &Source) -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(".cache"));
    let name: String = src.url.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    cache.join("nami/repos").join(name)
}

/// Clone (or update) the repo and apply it: this computer's system.nami
/// (its folder, named after it) to the computer (admins), and the
/// <login>/user.nami of whoever runs this to them.
pub fn run(root: &Path, arg: &str, flags: Flags) -> Result<()> {
    let (repo, computer) = split_computer(arg);
    let src = parse(&repo)?;
    let root_user = crate::profile::is_root();
    let mine = PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/nami");
    // A dry run, or root (who has no user file), reads a copy of its own.
    let scratch = flags.dry_run || root_user;
    let dir = if scratch {
        scratch_dir(&src)
    } else {
        mine.clone()
    };
    let git = Git::get(root)?;
    if scratch && dir.exists() && !dir.join(".git").exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    sync(&git, &src, &dir, !scratch)?;
    ignore_own_files(&dir);

    // This computer's folder (named after it): its system.nami, and yours in it.
    let host = crate::config::computer_dir(&dir, computer.as_deref(), crate::config::hostname().as_deref())?;
    let system = crate::oldnames::find_in(&host, crate::config::SYSTEM_FILE);
    let login = crate::profile::login().unwrap_or_default();
    let user = crate::config::person_file(&host, &login);
    let shown = |f: &Path| f.strip_prefix(&dir).unwrap_or(f).display().to_string();
    let mut extra: Vec<&str> = Vec::new();
    if flags.locked {
        extra.push("--locked");
    }
    if flags.upgrade {
        extra.push("--upgrade");
    }
    if flags.dry_run {
        extra.push("--dry-run");
    }
    let me = std::env::current_exe()?;
    let nami = |args: &[&str]| -> Result<()> {
        let status = Command::new(&me).args(args).args(&extra).status()?;
        ensure!(status.success(), "nami {} failed", args.join(" "));
        Ok(())
    };
    if system.is_file() {
        if Path::new("/etc/NIXOS").exists() && !crate::nami_os::here() {
            eprintln!("leaving out {}: on NixOS, the computer's side stays with your NixOS configuration", shown(&system));
        } else if !root_user && !crate::profile::is_admin() {
            eprintln!("leaving out {}: it's system.nami, for an admin (sudo nami apply {arg})", shown(&system));
        } else {
            eprintln!("the computer: {}", system.display());
            nami(&["system-use", "--from", &system.parent().unwrap().display().to_string()])?;
        }
    }
    if user.is_file() {
        if root_user {
            eprintln!("leaving out {}: it's a person's settings; run `nami apply {arg}` as that person, not root", shown(&user));
        } else {
            eprintln!("your settings: {}", user.display());
            if !scratch {
                link_user_file(&user)?;
            }
            nami(&["apply", &user.display().to_string()])?;
        }
    }
    if !scratch {
        eprintln!("{} is a checkout of {}: commit and push what you change there, and `nami apply {arg}` again on your other computers", dir.display(), src.url);
    }
    Ok(())
}

/// The system file `arg` names: a repo (one of its computers' folders:
/// `github:you/config/laptop`, or the only one), a config repo's folder (the
/// same), a computer's folder, or the file.
pub(crate) fn locate_computer(root: &Path, arg: &str) -> Result<PathBuf> {
    let (repo, name) = split_computer(arg);
    let dir = if is_repo(&repo) { checkout(root, &repo)? } else { PathBuf::from(arg) };
    if dir.is_file() {
        return Ok(dir);
    }
    if name.is_none() && crate::oldnames::is_file(&dir.join(crate::config::SYSTEM_FILE)) {
        return Ok(crate::oldnames::find_in(&dir, crate::config::SYSTEM_FILE));
    }
    Ok(crate::oldnames::find_in(&crate::config::computer_dir(&dir, name.as_deref(), None)?, crate::config::SYSTEM_FILE))
}

/// This distribution's name, as its os-release says it (`NAME`: Ubuntu,
/// Fedora Linux), when it has one that isn't namiOS's.
pub fn distro_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default();
    distro_name_of(&text)
}

fn distro_name_of(os_release: &str) -> Option<String> {
    os_release.lines().find_map(|l| l.strip_prefix("NAME=")).map(|v| v.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty() && !v.to_lowercase().starts_with("nami") && !v.to_lowercase().starts_with("tray"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distro_name_is_os_releases() {
        assert_eq!(distro_name_of("PRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=\"Ubuntu\"\n").as_deref(), Some("Ubuntu"));
        assert_eq!(distro_name_of("NAME=Fedora Linux\n").as_deref(), Some("Fedora Linux"));
        assert_eq!(distro_name_of("NAME=\"nami\"\n"), None);
        assert_eq!(distro_name_of("ID=arch\n"), None);
    }

    #[test]
    fn which_computer() {
        assert_eq!(split_computer("github:a/cfg/laptop"), ("github:a/cfg".into(), Some("laptop".into())));
        assert_eq!(split_computer("github:a/cfg/laptop@dev"), ("github:a/cfg@dev".into(), Some("laptop".into())));
        assert_eq!(split_computer("github:a/cfg"), ("github:a/cfg".into(), None));
        assert_eq!(split_computer("github:a/cfg/x.nami"), ("github:a/cfg/x.nami".into(), None));
        assert_eq!(split_computer("https://h/cfg.git"), ("https://h/cfg.git".into(), None));
        // The one form: @ the ref, # the computer.
        assert_eq!(split_computer("github:a/cfg@dev#laptop"), ("github:a/cfg@dev".into(), Some("laptop".into())));
        assert_eq!(split_computer("git+https://h/cfg.git@dev#laptop"), ("git+https://h/cfg.git@dev".into(), Some("laptop".into())));
        assert_eq!(split_computer("https://h/cfg.git@dev"), ("https://h/cfg.git@dev".into(), None));
        // The earlier ones: a URL's #ref, the Installer's #computer@ref.
        assert_eq!(split_computer("https://h/cfg.git#dev"), ("https://h/cfg.git@dev".into(), None));
        assert_eq!(split_computer("git+https://h/cfg.git#laptop@dev"), ("git+https://h/cfg.git@dev".into(), Some("laptop".into())));
    }

    /// Layers pinned in nami's checkout of a repo stay pinned when it's
    /// updated (the repo's own pins win).
    #[test]
    fn layer_pins_survive_a_pull() {
        let Some(bin) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("git")).find(|p| p.is_file()) else { return };
        let tmp = std::env::temp_dir().join(format!("nami-pins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let git = |dir: &Path, args: &[&str]| assert!(Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@x", "-c", "commit.gpgsign=false"]).args(args).output().unwrap().status.success(), "git {args:?}");
        git(&tmp, &["init", "-q", "--bare", "origin.git"]);
        git(&tmp, &["clone", "-q", "origin.git", "work"]);
        let work = tmp.join("work");
        std::fs::create_dir_all(work.join("laptop")).unwrap();
        std::fs::write(work.join("laptop/system.nami"), "a\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "one"]);
        git(&work, &["push", "-q", "-u", "origin", "HEAD"]);
        let src = Source { url: format!("file://{}", tmp.join("origin.git").display()), git_ref: None };
        let g = Git { bin, from_store: false };
        let co = tmp.join("checkout");
        sync(&g, &src, &co, false).unwrap();
        let pin = |rev: &str| crate::layers::Pin { url: "https://github.com/a/os.git".into(), git_ref: None, rev: rev.into() };
        let mut lock = crate::lock::Lock { version: 1, ..Default::default() };
        lock.layers.insert("github:a/os".into(), pin("1111"));
        crate::lock::save(&co.join("laptop/nami.lock"), &lock).unwrap();
        sync(&g, &src, &co, false).unwrap();
        assert_eq!(crate::lock::load(&co.join("laptop/nami.lock")).unwrap().layers["github:a/os"], pin("1111"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// `nami usb <repo>` let a computer open the secrets in nami's checkout:
    /// said how to share it, or committed and pushed; then nothing's left.
    #[test]
    fn a_computer_allowed_reaches_the_repo() {
        if std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).all(|d| !d.join("git").is_file()) {
            return;
        }
        let tmp = std::env::temp_dir().join(format!("nami-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let tmp = tmp.canonicalize().unwrap();
        let git = |dir: &Path, args: &[&str]| assert!(Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success(), "git {args:?}");
        git(&tmp, &["init", "-q", "--bare", "origin.git"]);
        git(&tmp, &["clone", "-q", "origin.git", "work"]);
        let work = tmp.join("work");
        for (k, v) in [("user.name", "t"), ("user.email", "t@x"), ("commit.gpgsign", "false")] {
            git(&work, &["config", k, v]);
        }
        std::fs::create_dir_all(work.join("laptop")).unwrap();
        std::fs::write(work.join(".nami-recipients"), "ada age1a\n").unwrap();
        std::fs::write(work.join("laptop/system.nami"), "a\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "one"]);
        git(&work, &["push", "-q", "-u", "origin", "HEAD"]);
        let share = |files: &[PathBuf], answer| share_change(Path::new("/"), "file://x", &work, files, "laptop", "age1l", Some(answer)).unwrap();
        assert_eq!(share(&[], false), Shared::Nothing);
        // What `nami secret allow` changed: said how, and left as it is.
        std::fs::write(work.join(".nami-recipients"), "ada age1a\nlaptop age1l\n").unwrap();
        std::fs::write(work.join("laptop/system.nami"), "b\n").unwrap();
        let changed = [work.join(".nami-recipients"), work.join("laptop/system.nami")];
        assert_eq!(share(&changed, false), Shared::Told);
        // Still there on the next run (nothing new changed): said again.
        assert_eq!(share(&[], false), Shared::Told);
        // Yes: committed (only those) and pushed.
        std::fs::write(work.join("notes.txt"), "mine\n").unwrap();
        assert_eq!(share(&[], true), Shared::Pushed);
        let log = Command::new("git").arg("-C").arg(tmp.join("origin.git")).args(["log", "-1", "--name-only", "--format=%s"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "Let laptop open the secrets\n\n.nami-recipients\nlaptop/system.nami");
        assert_eq!(share(&[], false), Shared::Nothing);
        assert!(work.join("notes.txt").exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn sources() {
        assert_eq!(parse("github:ada/cfg").unwrap(), Source { url: "https://github.com/ada/cfg.git".into(), git_ref: None });
        assert_eq!(parse("github:ada/cfg@laptop").unwrap().git_ref.as_deref(), Some("laptop"));
        assert!(parse("github:ada/cfg/sub").is_err());
        assert!(parse("github:../x").is_err());
        assert_eq!(parse("git@github.com:ada/cfg.git#v2").unwrap(), Source { url: "git@github.com:ada/cfg.git".into(), git_ref: Some("v2".into()) });
        assert_eq!(parse("git@github.com:ada/cfg.git@v2").unwrap(), Source { url: "git@github.com:ada/cfg.git".into(), git_ref: Some("v2".into()) });
        assert_eq!(parse("git+https://h.org/cfg.git@v2").unwrap(), Source { url: "https://h.org/cfg.git".into(), git_ref: Some("v2".into()) });
        assert_eq!(parse("ssh://git@h.org/cfg.git").unwrap(), Source { url: "ssh://git@h.org/cfg.git".into(), git_ref: None });
        assert!(is_repo("git@example.org:me/cfg") && is_repo("file:///srv/cfg") && !is_repo("./me.nami") && !is_repo("/etc/nami/system.nami"));
    }

    #[test]
    fn same_repo_however_written() {
        assert!(same_repo("git@github.com:Ada/cfg.git", "https://github.com/ada/cfg"));
        assert!(same_repo("ssh://git@github.com/ada/cfg.git", "https://github.com/ada/cfg.git/"));
        assert!(!same_repo("https://github.com/ada/cfg", "https://github.com/ada/other"));
        assert!(same_repo("file:///srv/cfg", "file:///srv/cfg.git"));
    }

    #[test]
    fn own_files_stay_out_of_git() {
        let d = std::env::temp_dir().join(format!("nami-machine-{}", std::process::id()));
        std::fs::create_dir_all(d.join(".git/info")).unwrap();
        std::fs::write(d.join(".git/info/exclude"), "# git's own\n*.swp").unwrap();
        ignore_own_files(&d);
        ignore_own_files(&d);
        let ex = std::fs::read_to_string(d.join(".git/info/exclude")).unwrap();
        assert!(ex.starts_with("# git's own\n*.swp\n") && ex.contains("\n/age.key\n"));
        assert_eq!(ex.matches("/age.key").count(), 1);
        std::fs::remove_dir_all(d.join(".git")).unwrap();
        ignore_own_files(&d);
        assert!(std::fs::read_to_string(d.join(".gitignore")).unwrap().contains("/secrets.env"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
