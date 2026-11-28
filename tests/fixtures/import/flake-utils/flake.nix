{
  description = "A Rust web service with a Node frontend";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        python = pkgs.python313.withPackages (ps: [ ps.requests ps.rich ]);
        dev = pkgs.writeShellApplication {
          name = "dev";
          runtimeInputs = [ pkgs.cargo-watch ];
          text = "cargo watch -x run";
        };
      in
      {
        packages.default = pkgs.callPackage ./package.nix { };

        devShells.default = pkgs.mkShell {
          buildInputs = [ pkgs.openssl ];
          nativeBuildInputs = with pkgs; [ pkg-config ];
          packages = with pkgs; [
            rustc
            cargo
            rust-analyzer
            nodejs_22
            python
            jq
            dev
            (callPackage ./tools/migrate.nix { })
          ];

          RUST_LOG = "debug";
          DATABASE_URL = "postgres://localhost/dev";
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.openssl ];

          shellHook = ''
            export RUST_BACKTRACE=1
            echo "welcome to the dev shell"
          '';
        };
      });
}
