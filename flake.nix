{
  description = "Nami: a distro-independent package manager on top of the nixpkgs binary cache";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      lib = pkgs.lib;
      version = (lib.importTOML ./Cargo.toml).package.version;
    in {
      packages.${system} = rec {
        # The CLI: one fully static (musl) binary, as ./build.sh makes it.
        nami = pkgs.pkgsStatic.rustPlatform.buildRustPackage {
          pname = "nami";
          inherit version;
          src = lib.fileset.toSource {
            root = ./.;
            # (namios/: namiOS's part, built in when it's here: build.rs.)
            fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./build.rs ./src (lib.fileset.maybeMissing ./namios/src) ./examples/packages ./examples/projects ./examples/services ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          doCheck = false; # some tests run bash and read glibc's libc: `cargo test --release`
          # The bundle runtimes ride at the end of nami (after strip, which would drop it).
          postFixup = ''
            $out/bin/nami attach-bundle-runtime $out/bin/nami $out/bin/nami-bundle-runtime $out/bin/nami-bundle-runtime-net
          '';
        };

        default = nami;
      };

      # Everything the index, site and upload scripts use, without Docker.
      devShells.${system} = {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo rustc clippy rustfmt pkg-config
            (python3.withPackages (p: [ p.pillow p.pyyaml ]))
            curl jq xz zstd gnutar coreutils
            nix-eval-jobs
            imagemagick  # packaging/publish-icons.py: icons that came as other formats, made PNG
            brotli  # index/history.py reads releases' packages.json.br
          ];
        };
      };
    };
}
