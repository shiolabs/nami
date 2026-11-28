//! How nami talks to Nix: its store paths and their base-32 hashes (`base32`), a store
//! that Nix itself manages, when Nix is installed (`store`), and the part of the Nix
//! language that `nami import` reads (`expr`). nami's packages are nixpkgs', so these
//! formats are nami's too; nothing here needs Nix to be installed.
pub mod base32;
pub mod expr;
pub mod store;
