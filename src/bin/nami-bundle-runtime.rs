//! The start of every bundle `nami bundle` makes (src/bundle_runtime.rs):
//! this one can't fetch, so it stays small. Thin bundles start with
//! nami-bundle-runtime-net instead.

#[path = "../bundle_ask.rs"]
mod bundle_ask;
#[path = "../bundle_format.rs"]
mod bundle_format;
#[path = "../bundle_menu.rs"]
mod bundle_menu;
#[path = "../bundle_sandbox.rs"]
mod bundle_sandbox;
#[path = "../bundle_trust.rs"]
mod bundle_trust;
#[allow(dead_code)]
#[path = "../elf.rs"]
mod elf;
#[path = "../nix/base32.rs"]
mod base32;
// (Where nami's code looks for it: crate::nix::base32.)
mod nix {
    pub(crate) use super::base32;
}
#[path = "../bundle_runtime.rs"]
mod runtime;

mod fetch {
    use crate::bundle_format::{Meta, PathEntry};

    pub fn fetch(_: &Meta, p: &PathEntry, _: &std::path::Path, _: &[u8], _: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("{} isn't in this bundle, and its runtime can't fetch it", p.name)
    }
}

/// Updating needs the network: bundles made with --update-url start with
/// nami-bundle-runtime-net (src/bundle_update.rs). These do nothing.
mod update {
    use crate::bundle_format::Meta;
    use std::path::Path;

    pub struct Me<'a> {
        pub path: &'a Path,
        pub meta: &'a Meta,
        pub hash: &'a str,
        pub cache: &'a Path,
    }

    pub fn at_start(_: &Path, _: &Meta) -> Option<String> {
        None
    }

    pub fn command(_: &[String], me: &Me) -> anyhow::Result<()> {
        let _ = (me.path, me.hash, me.cache);
        anyhow::bail!("{} doesn't update itself (it was made without --update-url)", me.meta.name)
    }

    pub fn info(_: &Me) -> Vec<String> {
        Vec::new()
    }

    pub fn before_start(_: &Me, _: i32) {}

    /// (Only a bundle that updates itself has a key: it starts with the other runtime.)
    pub fn fingerprint(_: &str) -> Option<String> {
        None
    }

    pub fn trust_in(dir: &Path, meta: &Meta) -> crate::bundle_trust::Trust {
        crate::bundle_trust::check_in(dir, meta)
    }

    pub fn settle_in(dir: &Path, meta: &Meta) -> std::io::Result<crate::bundle_trust::Trust> {
        crate::bundle_trust::record_in(dir, meta)
    }
}

fn main() {
    runtime::main()
}
