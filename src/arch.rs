//! The kind of computer nami runs on: x86_64 (Intel and AMD) or aarch64 (ARM), and the names
//! Linux distributions give its programs' places: glibc's loader (/lib64/ld-linux-x86-64.so.2,
//! /lib/ld-linux-aarch64.so.1), Debian's library folder (/usr/lib/aarch64-linux-gnu), its
//! packages' architecture (amd64, arm64). The platform catalogs are for is `index::SYSTEM`.

/// As `uname -m` says it: x86_64, aarch64.
pub const ARCH: &str = std::env::consts::ARCH;
/// Whether it's an ARM computer (aarch64).
pub const ARM: bool = cfg!(target_arch = "aarch64");
/// Where programs built for other distributions ask for glibc's loader.
pub const LOADER: &str = if ARM { "/lib/ld-linux-aarch64.so.1" } else { "/lib64/ld-linux-x86-64.so.2" };
/// glibc's loader's file, in nixpkgs' glibc's lib/ (bundle_format::loader_name for another).
pub const LOADER_NAME: &str = if ARM { "ld-linux-aarch64.so.1" } else { "ld-linux-x86-64.so.2" };
/// Debian's and Ubuntu's folder for this computer's libraries: /usr/lib/<MULTIARCH>.
pub const MULTIARCH: &str = if ARM { "aarch64-linux-gnu" } else { "x86_64-linux-gnu" };
/// Debian's and Ubuntu's name for it: amd64, arm64.
pub const DEB: &str = if ARM { "arm64" } else { "amd64" };

#[cfg(test)]
mod tests {
    #[test]
    fn names_go_together() {
        assert_eq!(super::LOADER_NAME, crate::bundle_format::loader_name(super::ARCH));
        assert!(super::LOADER.ends_with(super::LOADER_NAME));
        assert!(super::MULTIARCH.starts_with(super::ARCH));
    }
}
