//! The start of a thin bundle (`nami bundle --thin`): the bundle runtime
//! (src/bundle_runtime.rs) that also fetches the parts the bundle leaves
//! out, checked (src/bundle_fetch.rs).

#[path = "../bundle_ask.rs"]
mod bundle_ask;
#[path = "../bundle_delta.rs"]
mod bundle_delta;
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
#[path = "../bundle_fetch.rs"]
mod fetch;
#[allow(dead_code)]
#[path = "../narinfo.rs"]
mod narinfo;
#[path = "../nix/base32.rs"]
mod base32;
// (Where nami's code looks for it: crate::nix::base32.)
mod nix {
    pub(crate) use super::base32;
}
#[path = "../bundle_runtime.rs"]
mod runtime;
#[path = "../bundle_update.rs"]
mod update;

/// What narinfo.rs needs of nami's store.
mod store {
    pub const STORE_DIR: &str = "/nix/store";
    #[cfg(test)]
    pub const NIXOS_CACHE_KEY: &str = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";
}

fn main() {
    runtime::main()
}
