---
title: "Changes and undo"
description: "Apply a file, look back at what changed, and go back to any earlier setup."
---

## Apply a change

Each apply builds your whole new setup next to the one you have, then switches to it at once. That setup is a change, and it has a number.

```sh
nami apply
```

```text
Downloaded 8 packages · 58 MB
Apps
  + ripgrep 15.2.0
  ~ firefox 150.0 → 150.0.2
Files
  + ~/.gitconfig
✓ Done in 12 s · change 13 · 58 MB more on disk · nami undo
```

A change holds your packages, dotfiles, desktop settings, services and `/etc` files, and the file it came from. If the file has a mistake, nothing changes. A package that can't be found is left out and named at the end.

## See what would change

Add `--dry-run` to see the difference without changing anything.

```sh
nami apply --dry-run
```

```text
Apps
  + htop 3.5.3
  + git 2.56.0
  − jq 1.8.2
Dry run · 3 changes · nothing was changed yet · nami apply
```

It still downloads what's needed, so the real apply is quick afterwards.

## Look back

`nami history` lists every change, newest first, with what it did to your apps; `●` marks the one in use.

```sh
nami history
nami diff 11 13      # what differs between two changes
```

```text
● 13  just now        + ripgrep  ~ firefox 150.0 → 150.0.2  · in use
  12  yesterday       + gimp
  11  3 days ago      − vlc

What a change did: nami diff 13 · Go back one: nami undo
```

## Go back

`nami undo` goes back to the change before this one, and puts your file back as it was then:

```sh
nami undo
```

```text
Apps
  − ripgrep 15.2.0
✓ Back at change 12 · user.nami and nami.lock are as they were then
  Go forward again with nami undo 13
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
-   - ripgrep
```

Its `nami.lock` goes back too, so the next `nami apply` doesn't redo what you undid. Undo always goes back to the change you made this one from: if you undo, then install something, then undo again, you're back where you were before the install, and what you undid first stays undone. `nami undo 11` goes to any change, earlier or later. Files you link with `source:` from your home aren't copied into changes, so keep those in git.


## Old changes go by themselves

Old changes keep their packages, so `nami undo` is instant. That takes disk space, so a change goes by itself once it's 30 days old, with the packages nothing else uses. The current change and the 3 newest always stay, whatever their age. `cleanup:` sets when:

```yaml file="~/user.nami"
cleanup: 7d                            # after a week
# cleanup: off                         # never by themselves
# cleanup: { older-than: 90d, keep: 5 }
packages:
  - ripgrep
```

It runs with the daily update, and when you next use nami. In `system.nami` it's for the computer's changes; on namiOS the version running and the one before it stay in the boot menu. When the disk gets full (less than 10% or 5 GB free), old changes go early, by the same rules. Removing an app says what it left behind:

```text
✓ Removed brave · from user.nami
  Undo with nami undo
  450 MB can be freed now with nami gc --old · or it goes by itself after 30 days
```

`history: 10` keeps only the newest 10, whatever their age, after every apply. Settings › Storage has the same choice, and Clean Up Now.

`nami gc` frees space when you ask:

```sh
nami gc --keep 5            # keep the newest 5
nami gc --older-than 30d    # keep the last 30 days
nami gc --old               # keep only the current change
nami gc                     # keep every change, remove only what none uses
```

```text
✓ Freed 2.4 GB · 327 packages no longer used · changes 1, 2 and 3 removed
```

`nami gc --now` keeps what `cleanup:` keeps (the newest 3 and the current one) and removes the rest of the computer's changes. Add `--user` for your own changes, without an administrator. `nami storage` shows what each part of nami takes and what cleaning up now frees; the nami app shows the same under **Installed › Storage**.

## Get the same apps on another computer

`nami apply` writes `nami.lock` next to your file. It records the exact build of each package and the catalog it came from. Commit it with your file.

```sh
nami apply --locked     # install exactly what the lock says, or fail
```

Use `--locked` in CI, or on a second computer that must match. The lock for `~/user.nami` is `~/.config/nami/nami.lock`, and the computer's is `/etc/nami/nami.lock`.

## Upgrade

A newer catalog changes nothing until you ask.

```sh
nami update --refresh     # get the newest catalog
nami upgrade --dry-run    # see what would move
nami upgrade              # move, and rewrite the lock
```

```json file="~/.config/nami/nami.lock" diff
  "catalog": {
    "channel": "nixos-unstable",
-   "release": "55bfb4d7437b",
+   "release": "8c1e2f90aa3d",
  …
  "packages": {
    "firefox": {
-     "name": "firefox-150.0.2",
+     "name": "firefox-151.0",
  …
```

A name alone moves to the newest version, `firefox: 150` to the newest 150.x, and `python3: 3.12.4` stays put. If you don't like the result, `nami undo`.

## Find edits made outside nami

`nami status` lists files you've edited but not applied, and `/etc` files someone changed by hand.

```sh
nami status
```

```text
changed    /home/ada/user.nami (not applied yet: nami apply)
```

nami never throws away a hand edit. If your file later changes that `/etc` file, the edited one is kept as `<file>.nami-backup`.
