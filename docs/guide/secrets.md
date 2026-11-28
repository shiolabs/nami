---
title: "Secrets"
description: "Put passwords and tokens in your nami files, encrypted, so the files can go in git."
---

## Add a secret

Write the value where it's used and tag it `!secret`.

```yaml file="~/user.nami"
env:
  GITHUB_TOKEN: !secret ghp_abc123
```

```sh
nami secret seal
```

```yaml file="~/user.nami" diff
  env:
-   GITHUB_TOKEN: !secret ghp_abc123
+   GITHUB_TOKEN: !secret YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IFgyNTUxOSBrUnZ1T2pMcXNh…
```

The sealed line is safe to commit. Your comments and layout stay. A file with a secret that isn't sealed yet can't be applied.

Secrets are encrypted with [age](https://age-encryption.org) for everyone in the repo's `.nami-recipients`. nami opens one only where it's used, never into `/nix/store`.

## Know where secrets can go

A secret is one value, text or a number. In a settings file, seal just the setting that's secret.

```yaml file="~/user.nami"
env:
  API_KEY: !secret sk_live_123
files:
  ~/.config/rclone/rclone.conf:
    s3:
      type: s3
      secret_access_key: !secret "has # and: in it"   # quote YAML's characters
  ~/.netrc:
    content: !secret "machine example.org login ada password hunter2"
    mode: "0400"
```

| File | Where |
|---|---|
| `~/user.nami` | `env:` values and `files:` |
| `project.nami` | `env:` values, and a service's `env:` and `exports:` |

In user.nami, secret variables and files are written for you alone each time you apply. A project's secrets go only into the environment of `nami shell`, `nami run` and `nami up` ([Projects](projects.md)).

## Get your key

`nami secret key` prints your public key, and makes one if you have none.

```sh
nami secret key
```

```text
made your key: /home/ada/.config/nami/age.key (back it up; it never goes into git)
age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p
```

nami also opens secrets with your SSH key, `~/.ssh/id_ed25519` or `id_rsa`. Back up `age.key`: without it, or another listed key, nobody can open your secrets.

## Let someone in

Give their public key to `allow`, in the repo, on a computer where you can open the secrets.

```sh
nami secret allow sam age1u8fhn2k5t2f6...    # their `nami secret key`
nami secret allow sam ~/sam.pub              # or their SSH public key
git commit -am "sam can open the secrets" && git push
```

```txt file=".nami-recipients" diff
  ada age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p
+ sam age1u8fhn2k5t2f6...
```

`allow` also seals every secret again, for everyone listed. `--remove` takes someone out, and nami lists the secrets they could open, so you can change each one.

## Edit or read a secret

```sh
nami secret edit user.nami                     # opens with secrets shown, seals on save
nami secret show user.nami                     # list them
nami secret show user.nami env.GITHUB_TOKEN    # print one
```

A secret you didn't change keeps its sealed text, so `git diff` shows only your edit.

## Never commit a plain secret

```sh
nami secret hook
```

```sh file=".git/hooks/pre-commit"
#!/bin/sh
# Refuses commits with secrets that aren't sealed (`nami secret hook`).
nami=$(command -v nami || echo '/nix/var/nami/bin/nami')
exec "$nami" secret check --staged
```

The hook refuses a commit with an unsealed secret. In CI, `nami secret check` does the same.

