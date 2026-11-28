---
title: "Your own packages"
description: "Build a program the catalog doesn't have, or your own version of one it has, from a short package.nami recipe."
---

## Build a program from source

A recipe is a `package.nami` in a folder of its own. nami builds it the way nixpkgs builds its packages, in a sandbox, and you don't need Nix.

```yaml file="packages/hello/package.nami"
name: hello
version: 2.12.1
source:
  url: https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz
  sha256: 8d99142afd92576f30b0cd7cb42a8dc6809998bc5d607d88761f512e26c7db20
```

```sh
nami build ./packages/hello
```

```text
built /nix/store/...-hello-2.12.1
```

hello uses `./configure && make && make install`, which the standard build already knows, so the recipe needs nothing more. Building it again does nothing: the same recipe always gives the same result.

## Write a recipe from a link

`nami recipe from-url` writes the recipe for you, from a GitHub, GitLab or Codeberg repository (its latest release), a release's page, or any tarball or zip. It works for Rust, Go, Python and Node.js projects, and for projects built with Meson, CMake, `configure` or `make`.

```sh
nami recipe from-url https://github.com/sharkdp/hexyl
nami build ./hexyl
```

```text
hexyl 0.17.0: https://github.com/sharkdp/hexyl/archive/refs/tags/v0.17.0.tar.gz
a Rust project, built with Cargo; its crates come as its Cargo.lock pins them
✓ Wrote hexyl/package.nami
  Check it, then build it: nami build ./hexyl
```

The libraries a project needs are looked up in the catalog and written into the recipe. The download's hash goes in `nami.lock` next to it, so there's none to write by hand. For source code you already have, `nami recipe new <folder>` does the same, and `nami recipe update ./hexyl` moves a recipe to the project's newest release.

## Install your package

Install it by its folder. It's built if needed, then installed.

```sh
nami install ./packages/hello
```

```yaml file="~/user.nami" diff
  packages:
    - ripgrep
+   - ./packages/hello
```

Any nami file can list the folder the same way. A plain `- hello` also finds `./packages/hello`, since nami looks in the nearest `packages/` folder before the catalog; `- nixpkgs:hello` is always the catalog's.

## Check a download's hash

Each download has a `sha256:`. A download that doesn't match stops the build and prints the hash it got.

```sh
sha256sum hello-2.12.1.tar.gz     # or put in any 64 hex digits and copy the real one from the error
```

A `path: ./src` source uses a folder next to the recipe instead, and needs no hash.

## Add libraries and build tools

Libraries go in `deps`, and tools needed only for the build go in `build-deps`. Listing `cmake` or `meson` there makes the build use it.

```yaml file="packages/zcat-lite/package.nami"
name: zcat-lite
version: "0.1.0"           # quote a version YAML would read as a number
source:
  path: ./src
build-deps: [cmake]
deps: [zlib]
cmake-flags: [-DGREETING=nami]
```

An entry can be a catalog name, one output of a package (`libgcc.lib`; `nami info` lists them), or another recipe's folder. Only what the result really uses is installed with it, so CMake doesn't come along.

## Change a build step

The standard build runs unpack, patch, configure, build, check, install and fixup. You replace a step only when it doesn't fit.

```yaml file="packages/greet/package.nami"
name: greet
version: "1.0"
source:
  path: ./src
patches: [./fix-paths.patch]          # applied with patch -p1
make-flags: [CC=cc]
install: |                            # replaces the install step
  install -Dm755 greet $out/bin/greet
```

`configure-flags`, `meson-flags`, `build`, `check` and `build-env` work the same way; [Options](https://namios.dev/docs/reference/options) has each. `build-env` is only for the build; a program's environment when it runs goes in `env:` in a nami file.

## Package a prebuilt program

Many apps come only as a finished program that looks for its libraries in `/usr/lib`. `autoPatchelfHook` points it at the libraries in `deps` instead.

```yaml file="packages/bat-bin/package.nami"
name: bat-bin
version: 0.24.0
source:
  url: https://github.com/sharkdp/bat/releases/download/v0.24.0/bat-v0.24.0-x86_64-unknown-linux-gnu.tar.gz
  sha256: 0faf5d51b85bf81b92495dc93bf687d5c904adc9818b16f61ec2e7a4f925c77a
build-deps: [autoPatchelfHook]
deps: [libgcc.lib]        # for libgcc_s.so.1
install: |
  install -Dm755 bat $out/bin/bat
```

If a library is missing, the build names it (`could not satisfy dependency libfoo.so.1`). Find its package with `nami search` and add it to `deps`. The [vscode example](https://namios.dev/docs/reference/examples#packages-vscode) is a full one.

A finished program is made for one kind of processor. To package it for x86_64 and ARM computers both, give a download for each under `source:`. Each computer builds from its own, and one that isn't listed says so.

```yaml
source:
  x86_64-linux:
    url: https://github.com/sharkdp/bat/releases/download/v0.24.0/bat-v0.24.0-x86_64-unknown-linux-gnu.tar.gz
    sha256: 0faf5d51b85bf81b92495dc93bf687d5c904adc9818b16f61ec2e7a4f925c77a
  aarch64-linux:
    url: https://github.com/sharkdp/bat/releases/download/v0.24.0/bat-v0.24.0-aarch64-unknown-linux-gnu.tar.gz
    sha256: …
```

## Package a .deb

For an app that's only in Ubuntu's archive or on its maker's site as a `.deb`, `nami recipe from-deb` writes the recipe for you.

```sh
nami recipe from-deb dctrl-tools                  # Ubuntu's package, by name
nami recipe from-deb ./vendor-app_2.1_amd64.deb   # a file you have
```

```yaml file="dctrl-tools/package.nami"
# dctrl-tools: Command-line tools to process Debian package information
# Ubuntu noble's package, by `nami recipe from-deb dctrl-tools`.
# Check it, then: nami build ./dctrl-tools
name: dctrl-tools
version: 2.24-3build3
source:
  url: https://archive.ubuntu.com/ubuntu/pool/main/d/dctrl-tools/dctrl-tools_2.24-3build3_amd64.deb
  sha256: …
deps:
  - glibc                # libc.so.6
  - tre                  # libtre.so.5
# Programs: grep-dctrl, sort-dctrl, tbl-dctrl, join-dctrl
```

It turns each library the app needs into a package in `deps`, and marks what it can't place `NOT FOUND`, so read the file before you build it. Pass `--release noble` to look in another Ubuntu release.

## Change a nixpkgs package

`extends:` rebuilds one of nixpkgs' packages with your changes. Everything it depends on still comes ready-built.

```yaml file="packages/jq-patched/package.nami"
extends: jq
name: jq-patched
patches: [./nami-help.patch]     # added to nixpkgs' own patches
```

Lists such as `patches` and `deps` are added to nixpkgs' own, and `source:` and the steps replace them. A new `version:` and `source:` isn't enough for Rust and Go packages, which also pin a hash of their dependencies.

## Replace a library everywhere

When a library has a security fix the catalog doesn't have yet, `replace:` puts your fixed library into every package that uses it, and builds only the library.

```yaml file="~/user.nami"
replace:
  libpng: ./packages/libpng-fix     # an extends: recipe with the patch, and no name:
packages:
  - gimp
```

```text
libpng replaced in 1 package: gimp
```

The two libraries' versions must be the same length (`1.6.43` to `1.6.44`, not `1.6.9` to `1.6.10`), and nami refuses a library that's missing anything the old one had. When the catalog has the fix, take the line out and run `nami apply --upgrade`.

## Set up the program with its package

A `package.nami` can also say how its program is set up, with the keys every nami file has, such as `services:` and `files:`.

```yaml
# packages/htop/package.nami: the catalog's htop, set up your way
files:
  /etc/htoprc: |
    hide_threads=1
```

A recipe with nothing to build is the catalog's package of the folder's name. Its setup goes underneath the file that lists it.

## Fix a build that fails

nami prints the last lines of the build's output and where the whole log is.

```text
  | main.c:12:10: fatal error: zlib.h: No such file or directory
build failed (exit status: 2); full log: /nami/logs/...-zcat-lite-0.1.0.log
```

Here zlib is missing from `deps`. `nami build -K` keeps the build folder so you can look inside, and `NAMI_BUILD_DEBUG=1` logs every command. Builds have no network and see only what they depend on; inside Docker they need `--privileged`.

## Keep your recipes

Put recipes in your [config repo](config-repos.md) under `packages/`, in a project next to its project.nami, or in a public repository that [shares them](projects.md#share-a-tool-from-a-project). `nami upgrade` builds them again. To build once and install everywhere, see [Share builds](share-builds.md).
