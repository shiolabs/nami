---
title: "Ship your app"
description: "Make your program one file that runs on Ubuntu, Fedora, Debian, Arch, openSUSE and other distributions, old and new, with nothing to install first."
---

What a bundle gives you, at a glance: [nami bundle](https://namios.dev/bundle).

## Bundle your project

In your project's folder, `nami bundle .` builds the program and writes one file that carries everything it needs, down to glibc.

```sh
cd myapp
nami bundle .
```

```text
project: Rust (Cargo.toml)
  name     myapp  (Cargo.toml)
  version  0.3.1  (Cargo.toml)
  program  bin/myapp
✓ myapp-x86_64 · 25 MB · runs on any Linux distribution (x86_64, glibc)
  101 MB unpacked · 38 parts · hash badc59ad9c245cb409a5e2e50056294fdd15098be6f2f510a514d91e4dbe049b

  Test it on 7 distributions: nami bundle --test ./myapp-x86_64
  Publish it, with updates: nami bundle . --update-url <where> --publish-dir release/
```

nami builds the project with the tools `nami run` gives it, so you don't need any of them installed. A folder of programs you've already built is bundled as it is.

| Project | nami looks for | What runs |
|---|---|---|
| Rust | `Cargo.toml` | the program cargo builds |
| Go | `go.mod` | its `package main` |
| C, C++ | `meson.build`, `CMakeLists.txt` | what the build installs |
| Python | `pyproject.toml` | an entry point from `[project.scripts]` or `[project.gui-scripts]` |
| Node.js | `package.json` | its `bin`, on Node.js |
| Electron | `package.json` with `electron` | its `main`, on Electron |
| Zig | `build.zig` | what `zig build` installs |
| Flutter | `pubspec.yaml` and `linux/` | `flutter build linux`'s app |
| .NET | a `.csproj` or `.fsproj` | `dotnet publish`'s app, on .NET |

## Bundle a Python, Node.js or .NET app

nami uses the version your project asks for, from the catalog: `requires-python`, `engines.node`, the `electron` version in your dependencies, `minimum_zig_version` or `TargetFramework`.

```sh
cd myapp
nami bundle .
```

```text
project: Python (pyproject.toml)
  name     myapp  (pyproject.toml)
  version  1.2.0  (pyproject.toml)
  runs     myapp = myapp.cli:main  ([project.scripts])
  python   3.14  (requires-python >=3.11)
  program  bin/myapp
```

The bundle carries the interpreter or runtime and what your app depends on, from PyPI or npm, as your lock file pins it (`uv.lock`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`). A Node.js or Electron app gets its `dependencies`, not its `devDependencies`. An app from `gui-scripts`, Electron or Flutter gets an app menu entry.

Say the icon where your project's files have no place for one: `[tool.nami]` in `pyproject.toml`, or `"nami"` in `package.json`. Electron apps' electron-builder icon is used too.

```toml file="pyproject.toml"
[tool.nami]
icon = "assets/myapp.svg"
```

## Test it on other distributions

`--test` runs the bundle on Ubuntu 20.04 and 24.04, Debian 12, Fedora, Arch, openSUSE Tumbleweed and Linux Mint, each in a clean container. An app with a window is started under X11, and nami checks that its window opens and draws.

```sh
nami bundle . --test                  # bundle it, then test it
nami bundle --test myapp-x86_64       # test a bundle you made before
```

```text
Testing myapp 0.5.0 on 7 distributions, each in a clean container (an app: its window, under X11 (Xvfb))
   Distribution                    glibc  Starts  Window  Draws  First start
✓  Ubuntu 20.04.6 LTS              2.31   ✓       ✓       ✓      6.5 s
✓  Ubuntu 24.04.5 LTS              2.39   ✓       ✓       ✓      6.5 s
✓  Debian GNU/Linux 12 (bookworm)  2.36   ✓       ✓       ✓      6.6 s
✓  Fedora Linux 45                 2.44   ✓       ✓       ✓      6.6 s
✓  Arch Linux                      2.44   ✓       ✓       ✓      6.4 s
✓  openSUSE Tumbleweed             2.44   ✓       ✓       ✓      6.7 s
✓  Linux Mint 22.3                 2.39   ✓       ✓       ✓      6.5 s

✓ myapp 0.5.0 runs on all 7 distributions
  The 7 test images are kept for next time · 6.0 GB in podman's storage · podman image ls 'nami-bundle-test-*' lists them
```

It needs podman, which runs without root: `nami install podman`. The first test downloads each distribution and keeps it, so later tests start in seconds. A command-line program is run with `--version`; give it other arguments with `--test-args`, pick distributions with `--test-on ubuntu:20.04,fedora`, and get the results as JSON with `--json`.

## Run the bundle anywhere

Send people the file. It runs on any x86_64 distribution with glibc, from Ubuntu 20.04 to today's, without nami, Nix or root.

```sh
chmod +x myapp-x86_64 && ./myapp-x86_64
```

Only what must match the computer comes from it: the graphics driver, fonts, locale and certificates. A download loses its permission to run, so give people [the install line](#let-people-install-it-with-one-line) too.

The first time an app with a window starts, it asks once whether to add itself to the person's apps, and says who made it:

```text
Add Myapp to your apps?
Myapp by Ada Lovelace. Updates are signed by Ada Lovelace (key 3F2A 9C01 5B7E 22D4).
                                                              [Not Now]  [Add]
```

It asks with a notification, or a dialog where the desktop has no buttons on its notifications, and starts the app at once either way. A command-line tool never asks.

## Bundle something else

`nami bundle` also takes a recipe, a catalog package, a program or a release tarball.

```sh
nami bundle ./packages/myapp                # a recipe folder
nami bundle jq                              # a package from the catalog
nami bundle ./target/release/myapp          # a program you built
nami bundle myapp-1.0-linux-x86_64.tar.gz   # a release tarball
```

## Set the name, icon and menu entry

nami takes the name and version from the project's files or the newest git tag, and the icon and app menu entry from the build or a `.desktop` file in the repo. Say what it can't work out in `project.nami`.

```yaml file="project.nami"
bundle:
  name: myapp
  version: 1.2.0
  program: myapp
  icon: assets/myapp.svg
  desktop: assets/myapp.desktop
```

For one run, use `--name`, `--app-version`, `--program`, `--icon` or `--desktop-file` instead.

## Bundle a project nami can't build

For a Makefile, autotools or a script, `build:` says how. Install into `$DESTDIR$PREFIX`, or point `output:` at the program.

```yaml file="project.nami"
packages: [gnumake]
bundle:
  build: make install PREFIX=$PREFIX DESTDIR=$DESTDIR
```

With a script, use `build: ./build.sh --release` and `output: dist/myapp`.

## Publish from GitHub

`nami-bundle-action` bundles your project in your release workflow and uploads the file to the release.

```yaml file=".github/workflows/release.yml"
name: Release
on:
  push:
    tags: ['v*']
permissions:
  contents: write
jobs:
  linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0       # so the version comes from the tag
      - uses: shiolabs/nami/action@main
```

Push a tag such as `v1.2.0`, and its release gets `myapp-x86_64` and a `.sha256` file. The name has no version, so `https://github.com/ada/myapp/releases/latest/download/myapp-x86_64` always points at the newest. The action also takes `path`, `what`, `args`, `appimage: true` and `thin: true` under `with:`.

To test the bundle on the other distributions in the same run, give the action an `id` and add a step after it. GitHub's Ubuntu runners have podman.

```yaml
      - uses: shiolabs/nami/action@main
        id: bundle
      - run: nami bundle --test ${{ steps.bundle.outputs.bundle }}
```

## Let the app update itself

A bundle can update itself, the way apps on a Mac do. You need a GitHub release or any web server.

```sh
nami bundle . --update-url github:ada/myapp --publish-dir release/
gh release create v1.1 release/*
```

Upload everything in `release/` with every release; people's copies download only the parts that changed. A bundle looks once a day, takes only an update signed with your key, and switches to it at its next start.

### Download only what changed

When a part changes, a copy can download only the difference from the version it has, then rebuild the part from it. `--publish-dir` makes these patches for you from the release before, which it finds at your `--update-url`:

```sh
$ nami bundle . --update-url github:ada/myapp --publish-dir release/
since 1.1.0: 3 of its 38 parts changed; 3 of them as patches, 1.2 MiB in place of 2.6 MiB
```

The patches are the `.patch.zst` files in `release/`; upload them with the rest.

The daily update check runs in the background, so it always uses the patches and saves the most data. When someone waits for an update (`--nami-bundle-update`, the install line, or Settings), the copy measures the connection and takes the faster way: patches on a slow connection, whole parts on a fast one. For example, FeatherPad 1.6.3 to 1.6.4 needs 72 MiB of patches in place of 139 MiB of parts.

A copy checks what it makes from a patch against your signed release, and downloads the whole part when it can't use one. Copies made with an older nami download whole parts, as before.

If the release before isn't online yet, or you keep it somewhere else, name it with `--previous`: its bundle file, the folder you published it from, or its URL.

```sh
nami bundle . --update-url github:ada/myapp --publish-dir release/ --previous myapp-1.1.0-x86_64
```

## Keep your signing key safe

The first `--update-url` makes your key, `~/.config/nami/bundle-signing.key`. Back it up and keep it secret: without it you can't update the bundles people have, and with it anyone can.

```sh
gh secret set NAMI_BUNDLE_SIGNING_KEY < ~/.config/nami/bundle-signing.key
```

```yaml
# a step in your release workflow
- run: nami bundle . --update-url github:${{ github.repository }} --publish-dir release
  env:
    NAMI_BUNDLE_SIGNING_KEY: ${{ secrets.NAMI_BUNDLE_SIGNING_KEY }}
```

In CI with no key given, `nami bundle --update-url` stops rather than make a key that would be lost.

### Change to a new key

To move to a new key while you still have the old one, hand it over. The old key signs the new one, and the copies people have follow it on their own.

```sh
nami bundle --rotate-key
gh secret set NAMI_BUNDLE_SIGNING_KEY < ~/.config/nami/bundle-signing.key
```

```text
fingerprint: 81C0 77AB 0F12 9D3E 4A6B 0C55 E1D2 7F90 (was 3F2A 9C01 5B7E 22D4 8A10 C3F9 0E6B 7D21)
```

Your next release is signed with the new key and carries the handover. People's copies update to it, new downloads and the install line take it, and a `user.nami` that names the old key gets the new one written in. The old key is kept as `~/.config/nami/bundle-signing-<its name>.key`.

If you've lost the old key, there's nothing to sign the handover. People's copies then refuse the new key, as they'd refuse anyone else's.

## Let people install it with one line

`--publish-dir` also writes `nami-install.sh`, the script behind a line people paste in a terminal. nami prints the line:

```sh
nami bundle . --update-url github:ada/myapp --publish-dir release/
```

```text
✓ release/ is ready to publish · upload every file in it to https://github.com/ada/myapp/releases/latest/download
  People install it with: curl -fsSL https://github.com/ada/myapp/releases/latest/download/nami-install.sh | sh
  A Download for Linux button for your page: release/nami-download-button.html
```

The script downloads the bundle over HTTPS, checks it against the SHA-256 of that release, puts it in `~/.local/bin` and adds it to the app menu. Run it again and the copy already there updates itself, taking only an update signed with the key it trusts. With nami on the computer, the app is kept in the person's `user.nami` too.

`release/` also gets `nami-download-button.html`: a Download for Linux button and the install line, styled to paste into your page as it is.

## Say who made it

Your name goes in the bundle with your key, so people see who signs the updates: on the first start, in `--nami-bundle-info`, and on namiOS's install page.

```yaml file="project.nami"
bundle:
  developer: Ada Lovelace
```

For one run, use `--developer "Ada Lovelace"`. nami doesn't guess it from git: the last commit's author is often someone else, and a wrong name beside a key is worse than none.

```sh
./myapp-x86_64 --nami-bundle-info
```

```text
developer: Ada Lovelace
signed:   updates are signed by Ada Lovelace (key 3F2A 9C01 5B7E 22D4), the key you trust for it
sandbox:  Runs in a sandbox: it can use the network and your Documents folder.
```

The computer trusts the key the first time the app runs or is installed. An update, or a new download of the same app, signed with another key is refused:

```text
myapp is signed with another key (81C0 77AB 0F12 9D3E) than the one you trusted before (3F2A 9C01 5B7E 22D4). It may not be from Ada Lovelace, so it wasn't installed.
```

So keep your key, or [hand it over](#change-to-a-new-key): a new one can't take over the copies people have on its own. Someone who knows you really changed it trusts the new one themselves, with the install line's `| sh -s -- --trust` or the new copy's `--nami-bundle-trust`.

## Run it in a sandbox

Say what your app may use, and it runs inside bubblewrap on computers that have it, with nothing else of the person's.

```yaml file="project.nami"
bundle:
  sandbox:
    network: true
    files: [documents, downloads]
    devices: [gpu, audio]
```

For one run, list them: `nami bundle . --sandbox network,documents,downloads,gpu,audio`. `--sandbox` alone gives it nothing beyond its window.

| Key | Names |
|---|---|
| `network` | `true` or `false` |
| `files` | `documents`, `downloads`, `music`, `pictures`, `videos`, `desktop`, `home` |
| `devices` | `gpu`, `audio`, `camera` |

It always has its window, the system's files read-only, the person's theme and fonts, the files it's opened with, and a home of its own in `~/.local/share/nami-bundle/sandbox/<app>`. Where the computer has xdg-dbus-proxy (it comes with Flatpak), it reaches the desktop's portals, so its Open and Save dialogs can reach any file the person picks.

On a computer without bubblewrap, it runs without the sandbox and says so once: your app still starts on every distribution. People who'd rather it didn't run then set `NAMI_BUNDLE_SANDBOX=require`. namiOS's install page shows which it'll be:

```text
Runs in a sandbox: it can use the network and your Documents folder.
Not sandboxed: it can use everything you can.
```

## Make a smaller bundle

`--thin` leaves out every part cache.nixos.org has, and the bundle downloads those on its first start.

```sh
nami bundle jq --thin     # 3.4 MiB instead of 10.4
```

Each part is checked against the hash the bundle names. A thin bundle needs the network on its first start; a full one works offline.

## Bundle for ARM computers

`--arch aarch64` makes a bundle for ARM computers, such as a Raspberry Pi 4 or 5 with a 64-bit system, from a package in the catalog. Nothing is compiled: the ARM build comes from cache.nixos.org.

```sh
nami bundle jq --arch aarch64     # writes jq-aarch64
```

A project is built for the computer nami runs on, so bundle it for ARM on an ARM computer, or on GitHub's `ubuntu-24.04-arm` runner. A bundle for the other kind of computer says so, rather than failing in some other way.

## Use the bundle's own options

A bundle's options start with `--nami-bundle-`, so they never clash with your program's.

```sh
./myapp-x86_64 --nami-bundle-info        # name, version, who made it, its sandbox, where it updates from
./myapp-x86_64 --nami-bundle-install     # add it to the app menu
./myapp-x86_64 --nami-bundle-verify      # check every part against its hash
./myapp-x86_64 --nami-bundle-updates off # stop it updating itself
```

`--nami-bundle-gl` says which graphics driver it uses, and `--nami-bundle-clean` removes parts no bundle needs any more. `NAMI_BUNDLE_ASK=0` keeps it from asking to be added to the apps.
With nami on the computer, an installed bundle is kept in the person's `user.nami`:

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - bundle:github:ada/myapp: { key: 3F2A 9C01 5B7E 22D4 8A10 C3F9 0E6B 7D21 }
```

Another computer with the same file, or one that gets it through Sync, downloads it from your releases and checks it against that key.

## Know how big it gets

A bundle carries every file the program refers to, exactly as the store has it, so nothing is left out. It's compressed, and unpacks once into one store all bundles share, in `~/.cache`.

| | Bundle | Unpacked |
|---|---|---|
| `jq` | 10.4 MiB | 37.6 MiB |
| `featherpad` (Qt 6) | 143 MiB | 729 MiB |
| `gnome-text-editor` (GTK 4) | 264 MiB | 964 MiB |

All bundles unpack into one shared store, so GTK or glibc is on the disk once, however many bundles carry it. On namiOS and NixOS, a part the system's own store has, byte for byte, is used from there and not unpacked at all.

## How it works

A bundle is a small static program followed by every store path the app needs, with their hashes. On the first start it unpacks what isn't there yet, replacing `/nix/store` in every file with a `/tmp` path of the same length that links to the shared store, so nothing needs rewriting. Then it finds the computer's graphics driver and starts the program with the bundle's own glibc; later starts take about 2 ms.

## Real apps, bundled

Eight open-source apps were bundled once on Ubuntu 24.04, then started on Ubuntu 20.04 and 24.04, Debian 12, Fedora 45, Arch, openSUSE Tumbleweed, Linux Mint 22.3 and namiOS.

| App | Linux from its makers | Bundle | `--thin` | Ran on |
|---|---|---|---|---|
| Open Surge (game) | Flathub or Snap | 225 MiB | 3.6 MiB | all eight |
| simp (Rust, wgpu) | `cargo install` | 24 MiB | 11.5 MiB | all but Ubuntu 20.04 |
| Strawberry (Qt 6) | a package per release | 358 MiB | 3.7 MiB | all eight |
| Rnote (GTK 4) | Flathub | 263 MiB | 3.6 MiB | all but Ubuntu 20.04 |
| Amberol (GTK 4) | Flathub | 267 MiB | 3.7 MiB | all but Ubuntu 20.04 |
| Element (Electron) | an apt repository | 416 MiB | 3.7 MiB | all eight |
| tmux | a source tarball | 14 MiB | 4.0 MiB | all eight |
| aria2 | source only | 22 MiB | 3.4 MiB | all eight |

On Ubuntu 20.04's and Debian 12's older software Mesa, Rnote's and Amberol's windows stay black on X11; a bundle made with `--mesa` draws. On Linux Mint, all eight opened their windows and drew on X11, which Mint uses; Element didn't open a window on Wayland there. `tests/showcase.sh` in nami's repository runs them all again.

## Know the limits

- **glibc only**, so not musl distributions like Alpine. ARM bundles are for command-line programs from the catalog for now, and don't update themselves yet.
- **The first start unpacks** what isn't there yet, a few seconds for a GTK app.
- **`/tmp` must be writable**, as it is almost everywhere.
- **It's sandboxed only if you say so,** and only where the computer has bubblewrap. Otherwise a bundle runs with the person's rights, so sign its updates. Bundle only software you may hand out.
