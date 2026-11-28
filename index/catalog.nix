# What the catalog lists: every package of one nixpkgs release that can be
# installed on x86_64-linux. nix-eval-jobs walks it (index/in-container.sh):
# a derivation is a package; a set is walked only when it says so
# (recurseForDerivations), so helpers, functions and nested variants inside
# a set aren't. What doesn't evaluate (broken, insecure, not for this
# platform) comes out as an error and is left out.
#
# nixpkgs already marks most package sets to be walked: haskellPackages,
# python313Packages and python314Packages, perl5Packages, texlivePackages,
# vimPlugins, ocamlPackages, rubyPackages, lua5x/luajitPackages, akku, sbcl,
# typst, kdePackages, libsForQt5, lxqt, pantheon, xfce, mate, gnomeExtensions,
# gst_all_1, php's, idris, agda, elm, chicken, beam2xPackages, rocq, cuda,
# linuxPackages... These are the ones it doesn't, that hold packages
# found nowhere else.
#
# usage: import ./catalog.nix { nixpkgs = <unpacked nixpkgs>; [only = [ "rPackages" ... ];] [system = "aarch64-linux";] }
#   only: just these top-level names (to try a few sets without the rest).
#   system: another platform's packages: ARM Linux's (aarch64-linux: nami there, and
#   `nami bundle --arch aarch64`), or a Mac's (aarch64-darwin, x86_64-darwin).
{ nixpkgs, only ? null, system ? "x86_64-linux" }:
let
  pkgs = import nixpkgs { inherit system; config.allowAliases = false; config.allowUnfree = true; };
  inherit (pkgs) lib;
  # (pkgs.recurseIntoAttrs is an alias, gone with allowAliases = false.)
  recurse = lib.recurseIntoAttrs;
  extra = {
    # The names people know them by (the same builds as python314Packages,
    # or whichever Python is python3, and perl5Packages). The other
    # Pythons' sets nixpkgs doesn't walk (python311/312/315, pypy3) are the
    # same packages for another interpreter: left out.
    python3Packages = recurse pkgs.python3Packages;
    perlPackages = recurse pkgs.perlPackages;
    # CRAN and Bioconductor.
    rPackages = recurse pkgs.rPackages;
    # GNU ELPA, NonGNU ELPA and MELPA, as Emacs sees them (one of each);
    # its own sets of them (melpaStablePackages...) are walked only if they
    # say so.
    emacsPackages = recurse pkgs.emacsPackages;
    # Coq's last release line (Rocq, its successor, nixpkgs walks itself).
    coqPackages_8_20 = recurse pkgs.coqPackages_8_20;
    cmdpack = recurse pkgs.cmdpack;
    gerbilPackages-unstable = recurse pkgs.gerbilPackages-unstable;
    # The 32-bit graphics drivers (packages/steam: drivers: graphics-32bit).
    pkgsi686Linux = if system == "x86_64-linux" then recurse { inherit (pkgs.pkgsi686Linux) mesa pipewire; } else { };
    # NVIDIA's closed kernel modules for older cards (packages/nvidia-legacy;
    # nixpkgs has them only as nvidia_x11_legacy535.mod). Kernel modules are
    # for the default kernel only (linuxPackages); nixpkgs walks the others.
    # (A Mac has no Linux kernel: nothing of them in its catalog.)
    linuxPackages = if lib.hasSuffix "-darwin" system then { } else pkgs.linuxPackages // { nvidia_x11_legacy535_modules = pkgs.linuxPackages.nvidia_x11_legacy535.mod; };
  };
  all = pkgs // extra;
in
if only == null then all else lib.getAttrs only all
