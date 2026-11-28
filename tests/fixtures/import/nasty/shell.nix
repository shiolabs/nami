# A shell.nix with the awkward bits: comments, let ... in, lib.optionals,
# string interpolation, inherit, a rec set, an overlay.
{ pkgs ? import <nixpkgs> { overlays = [ (import ./overlay.nix) ]; }
, lib ? pkgs.lib
, withFun ? true
}:

let
  /* block comment */
  inherit (pkgs) hello;
  py = pkgs.python3.withPackages (ps: with ps; [
    numpy # arrays
  ]);
  names = rec {
    greeting = "hello";
    full = "${greeting}, world";
  };
in
pkgs.mkShell {
  packages = [ hello ]
    ++ lib.optionals withFun (with pkgs; [ cowsay lolcat ])
    ++ lib.optionals pkgs.stdenv.isDarwin [ pkgs.darwin.apple_sdk.frameworks.Security ]
    ++ [ py pkgs.nodePackages.typescript ];
  nativeBuildInputs = [ pkgs.gnumake ];

  GREETING = names.full;
  MULTI = lib.concatStringsSep " " [ "a" "b" ];
  PROJECT_ROOT = "${./.}";

  shellHook = ''
    export PS1="(nasty) $PS1"
  '';
}
