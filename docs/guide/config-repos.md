---
title: "Config repos and sets"
description: "Keep the nami files for all your computers in one git repo, share what they have in common, and set up a new computer from it with one command."
---

Your nami files are plain text, so they belong in git. One repo holds every computer: a folder for each, a folder inside it for each person, and `shared/` for what they have in common.

```txt
nami-config/
├── .nami-recipients     # who can open the secrets
├── packages/            # your own package folders
├── shared/
│   ├── system.nami    # what every computer has
│   └── user.nami        # you, on every computer
└── laptop/              # named after the computer's hostname
    ├── system.nami
    ├── nami.lock
    └── ada/
        └── user.nami    # named after the login
```

Either file can be left out. On a distribution where nami doesn't manage the computer, `laptop/ada/user.nami` on its own is a fine repo.

## Start a repo


Make the repo in `~/.config/nami` (`git init` there), then move your `~/user.nami` into it and link it back:

```sh
mkdir -p ~/.config/nami/$(hostname)/$USER
mv ~/user.nami ~/.config/nami/$(hostname)/$USER/
ln -s ~/.config/nami/$(hostname)/$USER/user.nami ~/user.nami
```

nami keeps files that belong to this computer alone, such as your secrets key `age.key`, out of git for you. Back up `age.key` somewhere safe.

## Share what your computers have in common

A file builds on another by listing its path in `packages:`. What it lists goes underneath, and the file itself wins:

```yaml file="laptop/system.nami"
packages:
  - ../shared/system.nami     # what every computer has
  - steam                       # and this one's own
hostname: laptop
users:
  ada: { admin: true }
```

```yaml file="laptop/ada/user.nami"
packages:
  - ../../shared/user.nami
  - obs-studio
```

A file listed this way is a set. A set can list a set of its own, up to eight deep, but each file builds on one set at most, so it's always clear which one wins. Paths are read from the file they're written in.

## See the whole file

`nami show --flat` prints a file merged with everything it lists, each line marked with the file it came from:

```sh
nami show --flat laptop/system.nami
```

Lists are joined, maps are merged key by key, and a single value such as `timezone:` comes from the nearest file that sets it.

## Build on someone else's set

A set can live in anyone's git repo. List it by its address:

```yaml file="system.nami"
packages:
  - github:alice/os/system.nami   # a layer, pinned to a commit
  - htop
```

Every git address in nami has the form `repo[/path][@ref][#name]`, such as `github:alice/os/gaming.nami@v2` or `git+https://codeberg.org/bob/os.git#gaming.nami`. Private repos work over ssh or https the way git does for you.

A set can change anything on the computer, so the first time nami reads one, it pins it to the commit it found, in `nami.lock`. Commit the lock with the file. Read a set before you use it.

## Move a layer to its newest commit

`nami layers` lists the sets a file takes from git, each at its pinned commit:

```sh
nami layers
```

```text
github:alice/os/system.nami  1a2b3c4d5e6f  https://github.com/alice/os.git
```

`nami layers --update` moves each pin to the newest commit:

```sh
nami layers --update
```

```json file="laptop/nami.lock" diff
  "layers": {
    "github:alice/os": {
      "url": "https://github.com/alice/os.git",
-     "rev": "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d"
+     "rev": "7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f809"
    }
  }
```

`nami apply --upgrade` moves them too, along with everything else. Nothing else moves a pin, so a pinned set also works offline.


## Set up a computer from the repo

On a computer with nami, one command clones the repo into `~/.config/nami` and applies it:

```sh
nami apply github:ada/nami-config
nami apply github:ada/nami-config/laptop@main    # a computer's folder, on a branch
```

nami picks the folder named after this computer's hostname unless you name one. If you're an admin, that folder's `system.nami` becomes the computer's file. Your `<login>/user.nami` is linked as `~/user.nami` and applied. To install nami and set up in one go, see [Install nami](install.md).

## Change and push

From then on, nami goes by the files in the checkout, and `nami install` and `nami config` edit them there. Commit and push as usual:

```sh
cd ~/.config/nami
git add -A && git commit -m "laptop: add htop" && git push
```

On your other computers, run `nami apply github:ada/nami-config` again to pull and apply what changed.

## Check the repo before you push

`nami check` reads every computer's and every person's file in the repo, and what they list:

```sh
nami check ~/.config/nami
```

It exits with 1 on an error, so CI can run it on each push. Passwords and tokens can go right in the files, sealed with `!secret` ([Secrets](secrets.md)).
