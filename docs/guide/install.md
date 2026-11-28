---
title: "Install nami"
description: "Put nami on the distribution you already have, keep it up to date, and take it off again."
---

## Install nami

Run the installer in a terminal on Ubuntu, Fedora, Mint, Arch or any other distribution, on x86_64 or [ARM](#install-on-arm-linux). It asks for your password once and takes a minute or two.

```sh
curl -fsSL https://namios.dev/install | sh
```

Open a new terminal and try `nami install firefox`. Log out and back in once, so your app menu finds nami's apps. Then go on to [Your first apps](first-apps.md).

On a computer with a desktop, the installer also adds [the nami app](nami-app.md). Don't want the app? Install with `curl -fsSL https://namios.dev/install | sh -s -- --no-app`, or set `NAMI_NO_APP=1`. To remove the app later, run `sudo nami remove nami-gui`; to add it, run `sudo nami install nami-gui`.

The installer checks the release against its signed checksums. If a check fails, nothing is installed.

nami is alpha software: most things work, some may not yet, and you can undo every change. If something isn't working, run `nami report`. To take nami off your computer again, see [Uninstall nami](#uninstall-nami).

## Install on ARM Linux

nami runs on 64-bit ARM computers with Linux: a Raspberry Pi 4 or 5 with a 64-bit system, ARM servers like AWS Graviton and Ampere, and laptops with Snapdragon chips. Run the same installer. It picks the ARM version for you.

```sh
curl -fsSL https://namios.dev/install | sh
```

Everything nami does on Linux works on ARM too: apps from the catalog, your user.nami with services and settings, projects, Flathub apps that have an ARM version, programs built for other distributions (their arm64 downloads), and bundles. Packages come ready-built for ARM from the binary cache.

On a computer with a desktop, the installer adds [the nami app](nami-app.md) on ARM too. `--no-app`, `sudo nami remove nami-gui` and `sudo nami install nami-gui` work as they do on x86_64.

Some things aren't on ARM yet:

- A package the binary cache has no ARM build of says it isn't ready-built for ARM Linux. That includes apps that aren't free software, which nami builds on x86_64 computers. Your own recipes build as they do on x86_64, but recipes that use `extends:` don't yet.
- Earlier versions, like `nami install nodejs@22`, and app icons in `nami search` aren't in the ARM catalog yet.
- Bundles you make on ARM are for ARM computers. `--update-url` and `--appimage` are for x86_64 bundles so far, and `nami bundle --test` leaves out Arch Linux and Linux Mint, which have no ARM images.
- The offline installer and the .deb and .rpm downloads are for x86_64.
- A Raspberry Pi needs a 64-bit system, like Raspberry Pi OS (64-bit) or Ubuntu for Raspberry Pi.
- Asahi Linux on Apple silicon Macs uses 16 KB memory pages, and some programs from nixpkgs are made for 4 KB pages and don't start there. nami hasn't been tested on Asahi Linux, and the installer tells you when your computer uses pages other than 4 KB. On such a computer the installer leaves out the nami app, which hasn't been tested there either. You can still add it with `sudo nami install nami-gui`.

## Install on a Mac

nami runs on Macs with Apple silicon and on Intel Macs, with macOS 14 (Sonoma) or newer. Run the same installer in Terminal. It asks for your password once. On an account that isn't an administrator, macOS asks for an administrator's name and password instead.

```sh
curl -fsSL https://namios.dev/install | sh
```

Open a new Terminal window and try `nami install ripgrep`. Your apps, your user.nami and your projects work as they do on Linux:

| Works on a Mac | What to run |
|---|---|
| Apps from the catalog | `nami install`, `remove`, `search`, `info`, `list`, `upgrade`, `update` |
| Your user.nami | `nami apply`, `history`, `diff`, `undo`, with `packages:`, `env:`, `aliases:` and files in your home folder |
| Projects | `project.nami`, `nami shell`, `nami run` and its tasks, `nami env`, `nami hook` |
| Apps with a window | In Applications, in a folder named nami Apps |
| Mac apps from Homebrew's casks | `nami install cask:rectangle`, or `- cask:rectangle` in your user.nami |
| Everything else | Tab completion in zsh and bash, `nami self-update` |

Apps with a window, like Alacritty or IINA, go in your Applications folder, in a folder named **nami Apps**, so you can open them from Spotlight, the Dock or Finder like any other app. Apps you install for everyone with `sudo nami install --system` go in nami Apps in the Mac's Applications folder. When you remove an app, or undo the change that added it, nami takes it out of Applications too, and it leaves anything else you put in nami Apps alone. Each app is a full copy, so it takes its size again on your disk.

### Mac apps from Homebrew's casks

Most Mac apps, like Google Chrome, Visual Studio Code, iTerm2, Slack or Rectangle, come from their developers, not from nixpkgs. Homebrew lists them as casks, and nami installs them from that list. You don't need Homebrew.

```sh
nami search rectangle            # apps from Homebrew's casks come after the catalog's
nami install cask:rectangle
```

In your user.nami, write `- cask:rectangle` under `packages:`, as you'd write `- flathub:<app id>` on Linux. A plain name works too: `- iterm2` is nixpkgs' iTerm2 when the Mac catalog has it ready-built, and otherwise the cask of that name, and nami says which one it used. On Linux, nami leaves casks out and says so, so one user.nami works on both.

nami downloads each app from its developer and checks it against the checksum in Homebrew's cask. A few casks have no checksum, because the developer's link always has the newest version, like Google Chrome's. nami installs those only if Apple has notarized the app. The app goes in nami Apps, and macOS asks before you open it the first time, as it does for any app you download. nami keeps a copy of each app it installs, so undoing a removal doesn't download it again.

- Casks have only the newest version of an app, so `nami install cask:rectangle@0.80` doesn't work.
- `nami upgrade` moves casks to the newest version, and so does the daily update. Apps that update themselves, like Chrome or Rectangle, are left as they are.
- nami installs casks that are an app and nothing more. A cask that needs Apple's installer (a .pkg, like Zoom), its own installer, other casks, or a newer macOS says why when you try. `nami info cask:zoom` says it too.
- Command-line programs inside an app come with it: `code` for Visual Studio Code.

The installer also puts [the nami app](nami-app.md) in your Applications folder, as **nami**. It opens like any other app. Don't want the app? Install with `curl -fsSL https://namios.dev/install | sh -s -- --no-app`. To remove it, drag nami from Applications to the Trash, and updates leave it out. To add it later, run the installer again.

Some things work differently on a Mac, or aren't there yet:

- Every package comes ready-built from the binary cache. nami doesn't build packages on a Mac, so a package without a Mac build, an app that isn't free software, or your own recipe says it isn't ready-built for Mac. For many Mac apps, Homebrew's casks have one.
- `services:`, desktop `settings:`, Flathub apps and bundles are Linux features. nami leaves them out and says so, so one user.nami works on both.
- The nami app shows what nami does on a Mac: Discover, Installed, Your Setup, Projects and History. Sync and a project's services aren't on a Mac yet.
- The nami app isn't notarized by Apple yet. Installed with the installer, it opens without a warning. A copy downloaded with a browser is stopped the first time: open **System Settings > Privacy & Security** and click **Open Anyway** under the message about nami.
- Earlier versions, like `nami install nodejs@22`, aren't in the Mac catalog yet.
- Apps for everyone on the Mac need `sudo`: `sudo nami install --system vlc`. Everything for you works without it.

nami updates itself once a day on a Mac too, with the apps you installed for everyone and each person's apps from Homebrew's casks, and tells you in a notification when something changed. An app that's open at that time is updated at the next update after you quit it. If your Mac is asleep at that time, the update runs when it wakes. To update only when you ask, add `updates: manual` to /etc/nami/system.nami and run `sudo nami apply`. [Update nami](#update-nami) has more.

On an Intel Mac, packages come from nixpkgs 26.05, the last release that builds for Intel Macs. It's supported until the end of 2026.

nami keeps what it installs in `/nix`, on an APFS volume of its own named Nix Store, the way the Nix installer does. The volume shares your disk's free space, doesn't show in Finder, and is encrypted when FileVault is on. If Nix is already on your Mac, nami uses its `/nix` and adds nothing. macOS keeps the top of the disk read-only, so on a Mac nami's own files are in `/nix/var/nami`, on the same volume, not in `/nami`.

To uninstall nami from a Mac, choose **nami > Uninstall nami…** in the nami app, or run `sudo nami self-uninstall` in Terminal. It removes the Nix Store volume too, unless Nix uses it. Dragging the app to the Trash removes only the app. [Uninstall nami](#uninstall-nami) has the details.

## Set up from your config repo

If you keep your nami files in a [config repo](config-repos.md), the installer can apply it right away.

```sh
curl -fsSL https://namios.dev/install | sh -s -- --config github:ada/nami-config
```

`--config` takes what `nami apply` takes: `github:ada/nami-config`, `github:ada/nami-config@branch`, or any git URL. If that step fails, nami stays installed and you can run `nami apply` later.

## Read the script first

You can download the installer and look at it before you run it.

```sh
curl -fsSL https://namios.dev/install -o install-nami.sh
less install-nami.sh
sh install-nami.sh
```

## Install another way

| Way | How |
|---|---|
| `.deb` or `.rpm` | Download it from [Download](https://namios.dev/download), when the release has one, and open it. Removing the package removes nami. |
| Offline | `sudo sh nami-<version>-x86_64-linux.run`. Apps still download, so installing them needs a network. |
| NixOS | The same one-line installer, or import the release's `nixos/nami.nix` and run `sudo nixos-rebuild switch`. |
| From source | `./build.sh`, `sudo ./install.sh`, then `sudo nami update --from <catalog> --unsigned` for a catalog you built. |

On NixOS, nothing in your configuration changes and no rebuild is needed. nami shares the Nix store, and `nix-collect-garbage` leaves nami's apps alone.

## Complete commands with Tab

Press Tab while you type a nami command, and your shell shows what fits: commands, options, apps from the catalog, an app's versions, your project's tasks and services, the changes you can undo, and the keys in your nami file. zsh and fish show what each one is.

```text
$ nami install fire<Tab>
firefox      -- Browse the web
firefox-esr  -- Browse the web, supported longer
...
$ nami install firefox@<Tab>
firefox@160  -- 160.0, since Nov 12, 2026
firefox@159  -- 159.0.2, Oct 1 – Nov 12, 2026
```

The installer sets it up for bash, zsh and fish, if you have them. Open a new terminal after you install. In bash it needs the `bash-completion` package, and in zsh it needs `compinit` in your `~/.zshrc`, which most setups already have. On namiOS it works out of the box.

If you keep your own shell files, load it there instead:

```sh
eval "$(nami completions bash)"     # in ~/.bashrc
eval "$(nami completions zsh)"      # in ~/.zshrc, after compinit
nami completions fish | source      # in ~/.config/fish/config.fish
```

## Know who can change what

nami uses your distribution's admin group: anyone in `wheel`, `sudo` or `admin` is an admin.

```sh
nami install vlc                   # anyone: just for you, in ~/user.nami
nami install --system vlc    # admins: for everyone, in /etc/nami/system.nami
nami status                        # says which you are
```

Admins install for everyone without `sudo`: a small service, `nami-daemon`, makes the change as root after checking who's asking. `sudo` decides which file `nami apply` and `nami undo` mean: yours without it, the computer's with it.

```sh
nami apply          # your ~/user.nami
sudo nami apply     # the computer's, /etc/nami/system.nami
sudo nami undo      # the computer's last change
```

## Check what it needs

nami is tested on Ubuntu, Debian, Fedora, Arch, openSUSE, Alpine and Void, and runs on NixOS, on x86_64 or ARM (aarch64) processors. It also runs on Macs with macOS 14 (Sonoma) or newer. It needs `curl` or `wget`, and `sudo` once. Services in your files need systemd; on Alpine or Void, apps and files work.

Everything nami installs lives under `/nix`. On a read-only system like Fedora Silverblue or SteamOS, the installer mounts `/var/lib/nami/nix` on `/nix`. Every path nami touches is in [Files on disk](https://namios.dev/docs/reference/files-on-disk).

## Update nami

nami updates itself once a day, with the apps in /etc/nami/system.nami, and says so in a notification. It checks each new release's signature before it installs it. Your files stay as they are.

To update now, open nami and click **Updates**, then **Update Now**. Or run:

```sh
nami self-update            # nami alone; an admin, no sudo needed
sudo nami self-update       # anyone else
nami self-update --check    # only say whether there's a newer version
```

To update only when you ask, turn off **Install updates automatically** in **Updates**, or add one line to system.nami:

```yaml file="/etc/nami/system.nami" diff
  packages:
    - vlc
+ updates: manual
```

To update your own apps, see [Update your apps](first-apps.md#update-your-apps).

## Bring your Nix setup

nami installs from nixpkgs and the same binary cache as Nix, so the packages are the same. You describe them in YAML instead of the Nix language.

```sh
nami import ~/.config/home-manager/home.nix
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - ripgrep
+   - git:
+       ~/.gitconfig:
+         user:
+           name: Ada
+           email: ada@example.org
+ env:
+   EDITOR: nvim
+
+ # Not converted from home.nix (nami import):
+ #   xdg.mimeApps: no nami equivalent
```

```sh
nami import .                                    # a flake's dev shell or shell.nix → project.nami
sudo nami import /etc/nixos/configuration.nix    # NixOS → /etc/nami/system.nami
nami import ~/dotfiles --to user --dry-run       # show what it would write
```

Import merges into the file and applies nothing, so run `nami apply` next. What has no nami equivalent, like overlays or the bootloader, is listed at the end of the file. If you're leaving home-manager, remove it and switch first, so its links in your home go away.

| Nix | nami |
|---|---|
| `configuration.nix`, home-manager | `system.nami`, `~/user.nami` |
| a flake's dev shell, `flake.lock` | `project.nami`, `nami.lock` |
| `nixos-rebuild switch`, `home-manager switch` | `nami apply` |
| `nix develop`, direnv's `use flake` | `nami shell`, `nami hook` |
| generations, `--rollback` | `nami history`, `nami undo` |
| `overrideAttrs`, `stdenv.mkDerivation` | `extends:`, a [recipe](recipes.md) |
| `environment.etc`, `home.file`, `dconf.settings` | `files:`, `settings:` |

nami and an installed Nix share one `/nix/store`. Set `NAMI_NIX=off` to make nami ignore Nix.

## Uninstall nami

Uninstalling removes nami itself: the nami app, every app and tool you installed with nami, and nami's store. To remove one app instead, click **Remove** on its page in the nami app, or run `nami remove <name>` (`nami uninstall <name>` does the same).

### In the nami app

- **On Linux:** open nami and click **Installed**. At the bottom, under **nami**, click **Uninstall nami…**.
- **On a Mac:** choose **nami > Uninstall nami…** in the menu bar. It's also at the bottom of **Installed**.

The app tells you what it removes and what stays. Click **Uninstall nami**, then enter your password. When it's done, click **Quit**.

On a Mac, dragging nami from Applications to the Trash removes only the app. nami and the apps you installed with it stay, so use **Uninstall nami…** to remove them.

### In a terminal

```sh
sudo nami self-uninstall
```

It tells you what it removes and what stays, then asks you to type `uninstall`. Anything else cancels. To skip the question in a script, add `--yes`.

If the `nami` command doesn't work any more, use the web uninstaller. It finds nami's uninstaller on your computer and asks the same way:

```sh
curl -fsSL https://namios.dev/uninstall | sh
```

In a script, run it with `sh -s -- --yes`.

### What stays

- Your nami files: `~/user.nami` and `~/.config/nami`, and on Linux the system's files in `/etc/nami`. Install nami again and they set your computer up as it was.
- Your home folder and everything in it.
- Anything in `/nix/store` that isn't nami's, like the packages of a Nix you installed yourself.

The services and `/etc` files that `nami apply` set up go, and files they replaced come back. On a Mac, the Nix Store volume goes too, unless Nix uses it. If you installed the `.deb` or `.rpm`, `sudo nami self-uninstall` removes the package for you.
