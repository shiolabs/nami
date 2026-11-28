//! `nami recipe`: a package.nami written for you, ready for `nami build`.
//!
//! - `from-deb` (from_deb.rs): a Debian package, its dependencies as
//!   nixpkgs packages.
//! - `from-url` and `new` (from_source.rs): source code, at a URL (a
//!   GitHub, GitLab or Codeberg repo's latest release or tag, a release,
//!   any tarball) or in a folder; what kind of project it is and what its
//!   build files ask for (wants.rs), as the catalog's packages.
//! - `update` (update.rs): a recipe moved to its newest release
//!   (upstream.rs finds it), with every hash recorded again.
//!
//! names.rs maps what builds and programs ask for (a library's file, a
//! header, a pkg-config module, a command) to the catalog's packages, for
//! these and for what a failed build says to add (build_errors.rs).

mod from_deb;
mod from_source;
pub(crate) mod names;
mod update;
mod upstream;
pub(crate) mod wants;

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use from_deb::{from_deb, parse_depends, version_cmp, RELEASE};
pub use from_source::{from_source, Options};
pub use names::{deb_package, package_with_library, soname_package};
pub use update::update;

/// `path` relative to the directory `base`, if they share anything.
pub(crate) fn relative_to(path: &Path, base: &Path) -> PathBuf {
    let (p, b): (Vec<_>, Vec<_>) = (path.components().collect(), base.components().collect());
    let common = p.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common <= 1 {
        return path.to_path_buf();
    }
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &p[common..] {
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn download(store: &crate::store::Store, url: &str) -> Result<Vec<u8>> {
    crate::say!("fetching {url}");
    let mut data = Vec::new();
    store.http().get(url).call().with_context(|| format!("fetching {url}"))?.into_reader().read_to_end(&mut data).with_context(|| format!("downloading {url}"))?;
    Ok(data)
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// How a folder is shown in what nami says: `./ripgrep`, as typed.
pub(crate) fn shown_dir(dir: &Path) -> String {
    let cwd = std::env::current_dir().ok();
    let rel = match cwd.as_deref().and_then(|c| dir.strip_prefix(c).ok()) {
        Some(r) if r.as_os_str().is_empty() => return ".".into(),
        Some(r) => r.to_path_buf(),
        None => return dir.display().to_string(),
    };
    format!("./{}", rel.display())
}
