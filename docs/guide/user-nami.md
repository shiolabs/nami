---
title: "user.nami"
description: "Keep your apps, dotfiles, environment, shell and desktop settings in one file, ~/user.nami."
---

## Write your file

`~/user.nami` is yours: what you install, how your programs are set up, and your shell. It works the same on any distribution and on namiOS.

```yaml file="~/user.nami"
packages:
  - ripgrep
  - git:
      ~/.gitconfig:                  # git's settings, under git
        user: { name: Ada, email: ada@example.org }
env:
  EDITOR: nvim
aliases:
  ll: ls -la
```

```sh
nami apply
```

You can also keep it at `~/.config/nami/user.nami`. Each apply is a [change](changes.md) you can undo, dotfiles included.

## Which file to use

nami reads three files with the same keys. The file decides whose setup it is.

| File | Where it lives | Who changes it | What it's for |
|---|---|---|---|
| `user.nami` | `~/user.nami` | you | your apps, dotfiles, shell and desktop settings |
| `system.nami` | `/etc/nami/system.nami` | admins | apps for everyone, services, `/etc` files ([more](system-nami.md)) |
| `project.nami` | a project's folder | anyone with the code | a project's tools, tasks and services ([more](projects.md)) |

`nami apply` applies your `~/user.nami`, and `nami undo` takes back your last change, admin or not. `sudo nami apply` and `sudo nami undo` are for the computer's file, system.nami. `nami apply <file>` applies just that file.

## Keep a dotfile

Put a program's settings under its package, so they go when the package goes. Nested YAML is written in the file's own format.

```yaml file="~/user.nami"
packages:
  - vim:
      ~/.vimrc: |                    # text, written as it is
        set number
  - alacritty:
      ~/.config/alacritty/alacritty.toml:   # settings, written as TOML
        font: { size: 12 }
files:
  ~/notes/todo.md: |                 # a file that belongs to no program
    - water the plants
```

nami writes JSON, TOML, YAML, INI, gitconfig and `.env` files from settings. Anything else, like a vimrc or a shell script, is text. Every path starts with `~/`.

## Link a folder from your repo

`source:` takes a file or folder next to your nami file.

```yaml file="~/.config/nami/user.nami"
packages:
  - neovim:
      ~/.config/nvim: { source: ./files/nvim }
```

When the source is inside your home, nami links straight to it, so an edit is live at once and ready to commit. A file already at that path is moved to `.nami-backup`, never deleted.

## Let an app change its settings file

Dotfiles are read-only links. For a file an app saves to itself, like VS Code's, mark it `mutable`.

```yaml file="~/user.nami"
packages:
  - vscode:
      ~/.config/Code/User/settings.json:
        mutable: true
        content:
          editor.fontSize: 14
```

When both you and the app change it, nami merges them setting by setting, and your file wins where both changed the same one. `nami home diff` shows what apps changed, and `nami home keep` writes it into your file:

```sh
nami home keep
```

```yaml file="~/user.nami" diff
  packages:
    - vscode:
        ~/.config/Code/User/settings.json:
          mutable: true
          content:
-           editor.fontSize: 14
+           editor.fontSize: 16
+           workbench.colorTheme: Solarized Light
```

To start managing a settings file that's already there, adopt it:

```sh
nami home adopt ~/.config/zed/settings.json
```

```yaml file="~/user.nami" diff
  …
+ files:
+   ~/.config/zed/settings.json:
+     mutable: true
+     content:
+       theme: One Dark
+       ui_font_size: 16
```

## Set your environment and aliases

`env:` reaches bash, zsh, fish and your desktop session. `aliases:` work in all three shells.

```yaml file="~/user.nami"
env:
  EDITOR: nvim
  PATH: $HOME/.local/bin:$PATH       # refers to the variable before
aliases:
  ll: ls -la
  gs: git status
```

A new terminal has them at once. Apps you start from the menu get them after you log in again. For a token, use a [secret](secrets.md).

## Add lines to your shell's startup

`shell:` holds lines run at the end of each interactive shell's startup, like prompts and plugins.

```yaml file="~/user.nami"
packages:
  - starship
  - zsh-autosuggestions
shell:
  zsh: |
    source "$(nami_share zsh-autosuggestions/zsh-autosuggestions.zsh)"
    eval "$(starship init zsh)"
  bash: |
    eval "$(starship init bash)"
```

`nami_share` finds a package's `share/` files, so a plugin's path survives updates. Applying adds one marked line at the end of `~/.bashrc` and `~/.zshrc`, and leaves your own lines alone:

```sh file="~/.zshrc" diff
  …
+ [ -r "$HOME/.config/nami/shell.zsh" ] && . "$HOME/.config/nami/shell.zsh"  # added by nami (home.shell)
```

## Choose the shell your terminals open

```yaml file="~/user.nami"
packages:
  - zsh:
      login-shell: true    # or fish
```

Terminals open zsh without `chsh` or root. Your login shell stays bash, so if zsh is ever missing, you can still get in.

## Set desktop settings

GNOME and GTK apps keep their settings in GSettings. List them by schema under `settings:`.

```yaml file="~/user.nami"
settings:
  org.gnome.desktop.interface:
    color-scheme: prefer-dark
    clock-format: 24h
packages:
  - gnome-text-editor:
      org.gnome.TextEditor:          # an app's settings, under the app
        show-line-numbers: true
```

nami sets them again on every apply, so take a value out of the file to keep your own. To find a setting, run `dconf watch /` and flip the switch in the app. Over ssh, settings wait for your desktop session.

KDE, niri and sway keep their settings in files, so write those as dotfiles, `mutable` when the desktop saves to them.

## Run a service for yourself

A service in user.nami runs as you, in your session.

```yaml file="~/user.nami"
packages:
  - syncthing
services:
  - syncthing.service    # the package's own unit
```

It starts when you apply and each time you log in. How to write your own is in [system.nami](system-nami.md#run-a-service).

## Change your file from the command line

These commands keep your file's comments and layout.

```sh
nami install ripgrep              # add it and apply
nami config add packages tree     # add it, apply nothing
nami config set env.EDITOR nvim   # change one value, apply nothing
```

```text
✓ env.EDITOR is nvim in user.nami · nami apply to use it
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - ripgrep
+   - tree
  env:
-   EDITOR: vim
+   EDITOR: nvim
```

`nami config remove packages tree` and `nami config unset env.EDITOR` take them out again, and `nami config get packages` prints what's there. A key with dots in it goes in brackets: `nami config set 'files["~/.gitconfig"].user.name' Ada`. With `sudo`, the same commands change the computer's file, system.nami. Your first `nami install` writes a `~/user.nami` with the apps you already had.

## Check your file

`nami check` finds mistakes without changing anything, and `nami apply` runs it first.

```sh
nami check ~/user.nami
```

```text
user.nami:3: error: fiefox isn't in the catalog (did you mean `firefox`?)
```

It exits with 1 on an error, so CI can run it. Set up [your editor](editor.md) to see mistakes as you type.

## See the whole file

Your file can list other files. `nami show --flat` prints it merged, with where each value came from.

```sh
nami show --flat ~/user.nami
```

```text
packages:
  - ripgrep  # work.nami
  - htop  # user.nami
```

To start a file from apps you installed before you had one, `nami export` prints them as a nami file. How files build on each other is in [Config repos and sets](config-repos.md).
