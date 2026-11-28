---
title: "system.nami on your distribution"
description: "As an administrator, install apps for everyone, run services and write files in /etc from one file, /etc/nami/system.nami."
---

`/etc/nami/system.nami` is the computer's file: what everyone on it gets. On Ubuntu, Fedora or any other distribution it holds apps, services and `/etc` files, and the distribution keeps its own users, kernel and settings.

## Install an app for everyone

Anyone in your distribution's admin group (`sudo`, `wheel` or `admin`) can change the computer's file. Run nami as yourself, not with `sudo`:

```sh
nami install --system htop
```

```yaml file="/etc/nami/system.nami" diff
  packages:
+   - htop
```

nami's daemon asks for your password and makes the change as root, and everyone on the computer has the app. `nami status` tells you whether you're an admin.

## Edit the file, then apply

The command above only writes a line in the file. You can write it yourself:

```yaml file="/etc/nami/system.nami"
packages:
  - htop
  - steam          # not free software: nami builds it here the first time
```

```sh
sudo nami apply
```

Without `sudo`, `nami apply` is your own `~/user.nami`. Each apply is a [change](changes.md) you can undo, with `sudo nami undo`.
Keys for a whole system, such as `users:`, `hostname:` and `timezone:`, are skipped here with a warning: your distribution keeps its own.

## Run a service

List a service under `services:` and `sudo nami apply` turns it on:

```yaml file="/etc/nami/system.nami"
services:
  - syncthing@ada          # the unit the package ships, for ada
  - tailscaled:
      package: tailscale   # the unit isn't named after its package
  - cups.socket            # a unit by its whole name
```

nami installs each package with its unit and starts it with the computer. Take a line out and apply, and the service stops and its unit goes. Services need systemd; without it, nami sets up everything else and says so.

The same entries in your `~/user.nami` run as your own services, in your session (`systemctl --user`). To keep them running when you're logged out, run `loginctl enable-linger` once.

### Change a package's service

Settings under an entry go into a drop-in, and the package's own unit stays as it is:

```yaml file="/etc/nami/system.nami"
services:
  - syncthing@ada:
      env: { STNORESTART: "1" }
      after: [network-online.target]
      restart: always
```

Other keys are `enable: false`, `description:`, `dir:` and `user:`. Every key is in [the options reference](https://namios.dev/docs/reference/options#services-package).

### Run your own program

Give an entry `run:` and nami writes the unit for you:

```yaml file="/etc/nami/system.nami"
services:
  - myapp:
      run: myapp-server --port 8080     # one command line
      env: { RUST_LOG: info }
      after: [network-online.target]
      /etc/myapp/config.toml:           # its own file: it restarts when this changes
        port: 8080
```

It runs as a user made up for it, with `/var/lib/myapp` for its data, starts with the computer and restarts when it fails. Give `user:` to run it as someone else.

### Run a script on a schedule

`script:` runs several lines with `bash -e`, and `at:` starts it on a schedule instead of with the computer:

```yaml file="/etc/nami/system.nami"
services:
  - backup:
      script: |
        cd /srv
        tar czf /backup/srv.tgz .
      at: daily                         # or "Mon..Fri 18:00", "*-*-* 04:00"
      systemd:                          # any other systemd setting, by section
        Service:
          Nice: 10
        Timer:
          RandomizedDelaySec: 15min
```

A run missed while the computer was off happens when it's back on. `systemd:` can set anything except `ExecStart`.

### See what apply will do

`--dry-run` shows which units nami writes, starts and restarts, without changing anything:

```sh
sudo nami apply --dry-run
```

```text
write /etc/systemd/system/myapp.service
start myapp.service
restart syncthing@ada.service (/etc/syncthing/config.xml changed)
```

A service whose unit didn't change keeps running. Units go in `/etc/systemd/system` as usual, so `systemctl status myapp` works. Edit them through your file, not by hand: nami puts its own copy back at the next apply.

## Write a file in /etc

Most programs read their settings from `/etc`. Write those files under `files:`:

```yaml file="/etc/nami/system.nami"
files:
  /etc/ssh/sshd_config.d/10-nami.conf:
    content: "PasswordAuthentication no\n"
    restarts: sshd.service         # restarted when the file changes
  /etc/docker/daemon.json:         # YAML, written as JSON
    log-driver: local
  /etc/sudoers.d/90-ops:
    content: "%ops ALL=(ALL) ALL\n"
    mode: "0440"
```

nami copies each file into place, owned by root, and moves a file that was already there to `.nami-backup`. `reload:` runs a command when a file changes. A file can also be copied from your folder with `{ source: ./certs }`.

nami won't write `/etc/passwd`, `/etc/shadow`, `/etc/group`, `/etc/sudoers`, `/etc/fstab` or `/etc/nami`, because a mistake there can lock everyone out. Use a file under `/etc/sudoers.d/` instead. Every form is in [the options reference](https://namios.dev/docs/reference/options#files).
