//! Bundles (`nami bundle`, `attach-bundle-runtime`) and downloaded apps (.deb, .rpm,
//! AppImage, bundles: `install <file>`, `package-info`, `open-package`, `file-apps`).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::store::Store;
use crate::{bundle, bundle_app, bundle_format, bundle_sandbox, bundle_test, foreign, profile, style};

/// `nami install bundle:...`: bundles, from where each updates.
pub(crate) fn install_bundles(packages: Vec<String>) -> Result<()> {
    anyhow::ensure!(!profile::is_root(), "a bundle is each person's own app: install it as yourself, not as root");
    for p in &packages {
        let (source, key) = bundle_app::parse_entry(p).ok_or_else(|| anyhow::anyhow!("{p}: a bundle is bundle:github:<owner>/<repo>, or bundle:https://<the folder it updates from>"))?;
        bundle_app::install_source(&foreign::Places::here(), &source, key.as_deref(), true)?;
    }
    Ok(())
}

/// `nami remove` of apps from downloads (.deb, .rpm, AppImage, bundles).
pub(crate) fn remove_downloads(packages: Vec<String>) -> Result<()> {
    let places = foreign::Places::here();
    for p in &packages {
        if let Some(rec) = foreign::find_installed(&places, p) {
            bundle_app::remove_any(&places, &rec)?;
        }
    }
    Ok(())
}

/// `nami package-info` of a bundle.
pub(crate) fn bundle_info(file: PathBuf, json: bool) -> Result<()> {
    let places = foreign::Places::here();
    if json {
        println!("{}", bundle_app::info_json(&places, &file)?);
    } else {
        print!("{}", style::out().card(&bundle_app::said(&places, &file)?));
    }
    Ok(())
}

/// `nami package-info` of a download (.deb, .rpm, AppImage, tarball).
pub(crate) fn package_info(file: PathBuf, json: bool) -> Result<()> {
    let l = foreign::look(&foreign::Places::here(), &file)?;
    if json {
        println!("{}", serde_json::to_string(&l)?);
    } else {
        print!("{}", style::out().card(&foreign::said(&l)));
    }
    Ok(())
}

/// `nami bundle --verify`.
pub(crate) fn verify(file: PathBuf) -> Result<()> {
    let (meta, hash) = bundle::verify(&file)?;
    println!("{}", style::out().line(&format!("✓ {} is whole · {} {} · every part matches its hash {hash}", file.display(), meta.name, meta.version)));
    Ok(())
}

/// `nami bundle`.
pub(crate) fn bundle(store: &Store, what: Option<String>, out: Option<PathBuf>, program: Option<String>, mesa: bool, desktop: bool, level: i32, exclude: Vec<String>, appimage: bool, thin: bool, name: Option<String>, app_version: Option<String>, icon: Option<PathBuf>, desktop_file: Option<PathBuf>, update_url: Option<String>, publish_dir: Option<PathBuf>, previous: Option<String>, developer: Option<String>, sandbox: Option<String>, test: bool, test_on: Vec<String>, test_args: Option<String>, json: bool, arch: String) -> Result<()> {
    let what = what.context("what to bundle")?;
    let tests = bundle_test::Options { on: test_on, args: test_args, json };
    // A bundle made before: tested, as it is.
    if test && bundle_format::is_bundle(std::path::Path::new(&what)) {
        if !bundle_test::test(std::path::Path::new(&what), &tests)? {
            std::process::exit(1);
        }
        return Ok(());
    }
    let project = bundle::project::Hints { name, version: app_version, icon, desktop: desktop_file };
    let sandbox = sandbox.map(|s| bundle_sandbox::parse_list(&s)).transpose().map_err(|e| anyhow::anyhow!("--sandbox: {e}"))?;
    let opts = bundle::Options { what, out, program, mesa, desktop, level, exclude, appimage, thin, project, update_url, publish_dir, previous, developer, sandbox, arch };
    let file = bundle::bundle(&store, &opts)?;
    if test {
        if !bundle_test::test(&file, &tests)? {
            std::process::exit(1);
        }
    } else if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        // (Where it is, for a script; the card above says it on a terminal.)
        println!("{}", file.display());
    }
    Ok(())
}
