---
title: "Share builds"
description: "Build something once, sign it, and let your other computers download it instead of building it again."
---

## Make a signing key

Almost everything nami installs comes ready-built from cache.nixos.org, but your own recipes and apps that aren't free are built on each computer. To build them once, you first need a key to sign them with.

```sh
nami keygen ada-1
```

```text
ada-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=
```

The printed line is the public key, safe to share. The secret key stays in `~/.config/nami/keys/`; back it up and keep it private, since anyone with it can sign builds your computers trust.

## Push a build to a cache

`nami push` writes what this computer built, with everything it needs, into a folder, compressed and signed.

```sh
nami push ~/cache ./packages/mytool     # a recipe, built first if needed
nami push ~/cache                       # everything built for your current change
```

```text
✓ Pushed 1 path · to /home/ada/cache
```

What came from cache.nixos.org is skipped, since your other computers get it there too. The folder is a standard Nix binary cache.

## Serve the cache

The cache is plain files, so anything that serves files works.

```sh
rclone sync ~/cache r2:my-cache     # an object store, or any static web host
```

A shared folder, such as an NFS mount or a USB disk, works as it is. Nobody has to trust the host: every path is signed, and a computer refuses any path that doesn't match a key it lists.

Before you serve an app that isn't free from a public host, check its licence. Many don't let anyone but their maker hand them out.

## Use the cache on your other computers

An admin lists the cache in system.nami, with the public key `nami keygen` printed.

```yaml file="/etc/nami/system.nami"
caches:
  - url: https://cache.example.org
    key: ada-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=
  - url: /mnt/team-cache                                     # a folder works too
    key: team-1:Lk8xbQ0n3S7m2y5WvR1cHk9pZt4uJdF6gA8sE0oN2Yc=
packages:
  - ./packages/mytool       # downloaded, not built
```

```text
mytool: downloading from cache instead of building
```

A user.nami can only use caches the computer already lists, and a project.nami can't add one, so nobody can make the computer trust a key an admin didn't choose.

## Use another catalog

The catalog of every package nami can install is signed, and a computer takes only one signed by a key it trusts. An admin can change where it comes from.

```sh
sudo nami update --from https://example.org/menu/nixos-unstable --trust cat-1:...
```

`--from` takes a channel or one index file, and `--trust` adds the key it's signed with. `--unsigned` takes a catalog you built yourself, with no signature.

## Sign a catalog you built

If you build a catalog yourself (`index/build-index.sh` in nami's repository), sign it with a key from `nami keygen`.

```sh
nami catalog sign out/ --key cat-1     # writes a .sig next to each index
```

A signed catalog can name binary caches of its own, and nami trusts those along with it.
