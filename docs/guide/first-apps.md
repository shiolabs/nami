---
title: "Your first apps"
description: "Find, install, update and remove apps with nami, next to your distribution's own."
---

## Install an app

nami installs apps from nixpkgs, over 140,000 packages, most of them ready-built, next to apt or dnf.

```sh
nami install firefox
```

```text
Downloaded 297 packages · 1.5 GB
✓ Installed firefox 160.0 · in your user.nami · 2.3 GB more on disk · opens from the launcher
  Undo with nami undo
```

```yaml file="~/user.nami" diff
  packages:
+   - firefox
```

Firefox is in your app menu right away, and with your file, the same apps come back on another computer. Each app brings its own libraries, so it never clashes with your distribution's.

## Find an app

Search works offline, from the catalog on your computer. A `✓` marks what you have.

```sh
nami search photo
```

```text
  gimp        3.2.6   GNU Image Manipulation Program
✓ photoflare  1.6.13  Simple but powerful Cross Platform Image Editor

nami install gimp · nami info gimp
100 command-line tools and libraries too: nami search --all photo
```

Search shows apps first, and tools and libraries when no app matches or with `--all`. `nami update --refresh` gets the newest catalog.

`nami info` tells you what an app is, whether you have it, and how many versions there are.

```sh
nami info firefox
```

```text
firefox 160.0
Fast, Private & Safe Web Browser
MPL-2.0 · http://www.mozilla.com/en-US/firefox/
App · opens from the launcher

Not installed · nami install firefox
314 earlier versions, back to 2016 · nami versions firefox
```

```sh
nami search --category games   # office, graphics, audio-video, games, internet, coding…
nami info firefox --all        # all of its description, and its screenshots
nami list                      # what you have
```

## Install several, or remove one

```sh
nami install vlc gimp ripgrep
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - vlc
+   - gimp
+   - ripgrep
```

```sh
nami remove vlc
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
-   - vlc
    - gimp
    - ripgrep
```

Removing an app takes its settings out of your file too.

If an install fails, nami puts your file back as it was. The [nami app](nami-app.md) does the same on its Discover and Installed pages.

## Install for everyone

An admin can install an app for everyone on the computer, without `sudo`.

```sh
nami install --system vlc
```

```yaml file="/etc/nami/system.nami" diff
  packages:
+   - vlc
```

`sudo nami undo` takes it back. Packages that set up the computer, like docker or tailscale, always go in that file. See [system.nami](system-nami.md).

## Pin a version

In your file, a name alone means the newest version. Add a version to stay on one.

```yaml file="~/user.nami"
packages:
  - ripgrep            # the newest
  - firefox: 150       # the newest 150.x
  - python3: 3.12.4    # exactly this
  - nodejs: 20         # the newest 20.x
```

nami also looks in earlier nixpkgs releases, so old versions are ready-built too. `nami versions` lists them, newest first, with when each was current.

```sh
nami versions firefox
```

```text
firefox 315 versions, back to 2016

  160  160.0    since Nov 12, 2026     current
  159  159.0.1  Oct 28 – Nov 12, 2026  ✓ installed
  158  158.0.1  Oct 13 – Oct 28, 2026
  157  157.0.1  Sep 29 – Oct 13, 2026
  …

Pick one: nami install firefox@159, or in a nami file: firefox: 159
```

`nami versions python3` groups by minor version (3.13, 3.12), and `--all` lists every release. `nami search firefox@150` says whether there's a firefox 150 and how to install it. The version is read as you wrote it, so `3.10` stays 3.10.

## Python with its packages

List the Python packages you want under `with:`. You get one Python that has them, the way nixpkgs' `python3.withPackages` makes one.

```yaml file="~/user.nami"
packages:
  - python3:
      version: 3.13              # optional: the newest 3.13
      with: [numpy, requests, django]
```

`python3 -c 'import numpy'` works, `pip list` lists the packages, and their commands, such as `django-admin`, are on your `PATH`. The packages come from the catalog's set for that Python (`python313Packages`), along with the Python packages they need, so they always match the interpreter. Find a package's name with `nami search <word> --all`: `python313Packages.numpy` is `numpy` here. It works the same in system.nami and in a project's project.nami.

## Edit the file, then apply

You can edit `~/user.nami` in any editor and make the computer match it.

```sh
nami apply --dry-run   # show what would change
nami apply
nami undo              # go back to the change before
```

Every apply is a change you can undo. More in [Changes and undo](changes.md).

## Update your apps

nami doesn't update your apps on its own. When you want newer versions:

```sh
nami update --refresh       # get the newest catalog
nami upgrade --dry-run      # see what would get newer
nami upgrade
```

Pinned versions stay within what you picked, and `nami undo` takes an upgrade back. `sudo nami upgrade` updates the computer's apps, the ones for everyone; the daily update does that by itself. To update nami itself, see [Update nami](install.md#update-nami).

## Install an app from Flathub

Some developers publish their apps on Flathub. Install one by its ID.

```sh
nami search --flathub calculator
nami install flathub:org.gnome.Calculator
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - flathub:org.gnome.Calculator
```

Flathub apps need your distribution's `flatpak` package, and they're always just for you. `nami upgrade` updates them too.

## Install an app made with nami bundle

Some developers publish their app as one file made with [nami bundle](ship-your-app.md). Install it from where they publish it, or open a download of it.

```sh
nami install bundle:github:ada/myapp
nami install ./myapp-x86_64
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - bundle:github:ada/myapp: { key: 3F2A 9C01 5B7E 22D4 8A10 C3F9 0E6B 7D21 }
```

`key:` is the key its developer signs it with, written the first time. On another computer, `nami apply` downloads the app and installs it only if it's signed with that key. The app updates itself, and it's just for you. Installing or removing it is a change like any other, so `nami undo` takes it back.

## Allow or refuse apps that aren't free

Steam, Spotify, VS Code and other apps that aren't free software are allowed unless you say otherwise.

```yaml file="~/user.nami"
unfree: false     # refuse apps that aren't free software
packages:
  - firefox
```

These apps aren't in the public binary cache, so nami builds them on your computer, which takes a few minutes the first time. You can [share the build](share-builds.md) with your other computers.

## When an app isn't in the catalog

If nixpkgs renamed or dropped a package, `nami apply` leaves it out, says so, and sets up the rest.

```text
user.nami: leaving out oldtool: it isn't in the catalog (did you mean `newtool`?)
```

For an app nixpkgs doesn't have, write [your own package](recipes.md). For apps that come only as a `.deb` or a download, see [Apps from other distributions](other-distributions.md).

## When a graphical app shows nothing

The first time you install an app that uses the graphics card, nami also sets up drivers, fonts and languages for it. If an app shows nothing or complains about OpenGL, run `nami doctor`.

```sh
nami doctor
```

See [Troubleshooting](troubleshooting.md) for the rest.
