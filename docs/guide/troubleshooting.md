---
title: "Troubleshooting"
description: "Find out what's wrong, fix the messages you're most likely to see, get back to a setup that worked, and tell us."
---

Most of nami's messages already say what to run. This page covers the common ones.

## Find out what's wrong

Three commands look without changing anything:

```sh
nami status      # who you are to nami, your change, the catalog, services
nami doctor      # the store, graphics drivers, and whether services can run
nami check       # problems in your file, line by line
```

`nami status` names whose changes yours are, and says what needs doing, each with a `✓`, a `⚠` or a `✗`:

```text
ada · only you
  ✓ Change 13 is in use · 22 apps
  ✓ Catalog · 98,213 packages · updated 2 days ago
  ⚠ system.nami has changes not applied yet · sudo nami apply
```

`✗ No catalog yet` means nami has nothing to install from: `nami update --refresh` fetches one. In `nami doctor`, `⚠ Graphics drivers aren't set up` means apps with a window may not start: restart the computer, or run the installer again.

## Fix the daemon or the catalog

| What you see | What to do |
|---|---|
| `the nami daemon isn't running` | `sudo systemctl enable --now nami-daemon`. If it won't start, `journalctl -u nami-daemon` says why. |
| `the nami daemon closed the connection` | The daemon restarted. Run the command again. |
| `no package catalog here yet` | Run the installer again. |
| `... isn't signed` or `signed by "nami-1", which this machine doesn't trust` | Pass the key with `--trust`, or `--unsigned` for a catalog you built yourself. |
| `the signature by "nami-1" doesn't match` | The catalog was changed after it was signed. Don't use it; fetch it again from nami's site. |

Without systemd, start the daemon with `sudo rc-service nami-daemon start` (OpenRC) or `sudo ln -s /etc/sv/nami-daemon /var/service/` (runit).

## Fix a mistake in your file

`nami apply` checks the whole file first and changes nothing while something is wrong. Each message names the line:

```text
/etc/nami/system.nami:7: settings: org.gnome.desktop.interface has no key colour-scheme (did you mean `color-scheme`?)
```

| What you see | What to do |
|---|---|
| `` unknown key `pakages` (did you mean `packages`?) `` | Fix the name. If it's right, your nami may be too old: `nami self-update`. |
| `true or false: write false (in YAML, no is text)` | Write `true` or `false`, not `yes`, `no`, `on` or `off`. |
| `` `users:` only works on namiOS `` | A warning: your distribution keeps its own users, hostname and time zone. |
| `home settings are each person's own` | Move `settings:` and dotfiles into your `~/user.nami`. |
| `files: writes /etc, so it's only for system.nami` | Put `/etc` files in [system.nami](system-nami.md#write-a-file-in-etc). |
| `... is a project's file` | Use `nami shell` and `nami run` in the project, not `nami apply` ([Projects](projects.md)). |

## Fix an app that won't install

nami doesn't let one package hold up the rest: it leaves out what it can't install and names it at the end.

| What you see | What to do |
|---|---|
| `` leaving out fierfox: it isn't in the catalog (did you mean `firefox`?) `` | `nami search <word>` finds what it's called now. |
| `no binary cache has foo-1.2 (nixpkgs couldn't build it)` | nixpkgs' build failed this time. Run `nami update --refresh` later and apply again. |
| `not a version the catalog has` | `nami versions <name>` lists every version. |
| `ada isn't an admin ..., so can't change the computer's apps` | Install it for yourself, or ask an admin. To make ada one: `sudo usermod -aG sudo ada` (Ubuntu) or `sudo usermod -aG wheel ada` (Fedora, Arch). |
| `steam isn't free software, and your file says unfree: false` | Remove that line. Apps that aren't free are built on your computer, so they take longer. |
| `cache https://... isn't trusted on this machine` | Only an admin can add a cache, in system.nami's `caches:` ([Share builds](share-builds.md)). |

## Fix the lock

| What you see | What to do |
|---|---|
| `ripgrep isn't in nami.lock; apply without --locked` | Apply without `--locked`, then commit the new `nami.lock`. |
| `... builds something different from what nami.lock says` | A recipe changed. Apply without `--locked` to update the lock. |
| `the pinned catalog isn't on this computer` | `nami apply --upgrade` moves the lock to today's catalog. |

## Fix a failed build

A failed build names its log:

```text
error: build failed (exit status: 2); full log: /nami/logs/abc123...-mytool-1.0.log
```

Read the log, or keep the build folder to look inside with `nami build -K <recipe>` ([Your own packages](recipes.md)).

## Fix something else

| What you see | What to do |
|---|---|
| `none of your keys can open it` | Run `nami secret key` and send the output to someone who can already open it ([Secrets](secrets.md)). |
| `settings: skipped, no desktop session here` | Run `nami apply` from a terminal in your desktop, not over ssh. |
| `couldn't edit this file's layout in place safely` | `nami config` doesn't edit flow-style YAML or anchors. Edit that part by hand. |
| `couldn't update ~/.config/nami from the repo` | The pull clashes with edits you haven't committed. Sort it out with `git status` in `~/.config/nami`. |
| `... (it was changed by hand)` | nami put its own copy back and moved yours aside. Make the change in your nami file. |
| `nami's window couldn't open` | The app draws with Vulkan. `nami doctor` shows the graphics drivers, and the `nami` command does everything the app does. |
| A device ignores a new udev rule | Run `sudo udevadm trigger` for devices already plugged in. |

## Get back to a setup that worked

Every change can be undone:

```sh
nami undo          # the change before this one
nami history       # all of them
nami undo 7        # any change, back or forward
```

`nami undo` also puts your file and its `nami.lock` back, so the next apply keeps it ([Changes and undo](changes.md)).

## Read the logs

| What | Where |
|---|---|
| The daemon | `journalctl -u nami-daemon` |
| Builds | `/nami/logs/` |
| The computer's services | `journalctl -u <name>` |
| Your own services | `journalctl --user -u <name>` |
| A project's services | `nami logs` |

## Tell us

`nami report` asks what happened, then shows everything it will send before it sends it:

```sh
nami report
nami report --show       # only show what would be sent
```

It sends your text, nami's version, your distribution, `nami doctor`'s output, your last ten changes and your nami files. Values marked `!secret` are left out, but a token written plainly in a file would go along, so add `--no-files` if you have one.
