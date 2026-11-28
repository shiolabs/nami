---
title: "Sync"
description: "Keep your apps and settings the same on all your computers, and keep folders of files in sync."
---

## Sync your setup

Add an app on your laptop, and it's on your desktop too. nami keeps your `~/user.nami` in a private git repository of yours.

```sh
nami sync on                         # a private nami-setup repository on your GitHub
nami sync on github:ada/my-setup     # or a repository you choose
```

Run it on each computer. On the second one, the first sync brings in what the first one has, and sends back what this one has:

```yaml file="~/user.nami" diff
  packages:
    - htop
+   - firefox
+   - gimp
```

It works between namiOS and nami on any distribution. In the [nami app](nami-app.md), choose **Sync**, then **Keep This Setup on All Your Computers**.

Git signs in the way it already does for you, so no password goes in any file.

## Know when it syncs

Your setup syncs every 15 minutes, at login, and as soon as your file changes.

```sh
nami sync now       # sync right away
nami sync status    # your computers, the last sync, and what came in
```

Changes merge key by key and app by app, so an app added on each computer keeps both. When two computers change the same thing, the newer change wins. Each computer keeps its own displays, graphics card, backup drive and sound.

## Undo a sync

`nami sync status` numbers what each sync brought in.

```sh
nami sync undo 3    # take back what sync 3 changed, on all your computers
nami sync off       # stop syncing this computer
```

When you turn sync off, this computer keeps its apps and settings, and your other computers go on syncing.

## Sync folders of files

The `sync` package uses Syncthing to send files straight between your computers, encrypted, with no cloud and no account.

```yaml file="~/user.nami"
packages:
  - sync
files:
  ~/.config/nami-sync.conf:
    folders:
      Sync: ~/Sync            # the same name on each computer is the same folder
      Photos: ~/Pictures
    computers:                # your other computers, by Syncthing ID
      desktop: MFZWI3D-BONSGYC-YLTMRWG-C43ENR5-QXGZDMM-FZWI3DP-BONSGYY-LTMRWAD
```

Apply it, and Syncthing starts now and each time you log in. In the nami app, **Set Up Sync** on the Sync page does the same.

## Pair your computers

Each computer has an ID. List each of your other computers once in the file.

```sh
nami-sync id
```

```text
P56IOI7-MZJNU2Y-IQGDREY-DM2MGTI-MGL3BXN-PQ6W5BM-TBBZ4TJ-XZWICQ2
```

The laptop lists the desktop, and the desktop lists the laptop. Then every folder syncs to every computer listed. An Android phone with Syncthing-Fork can join too.

The file is the list: take a folder out and it stops syncing, but its files stay. `nami-sync status` shows each folder's state, and Syncthing's own page is at http://127.0.0.1:8384.
