---
title: "The nami app"
description: "Use nami without a terminal: find apps, keep your setup, run projects and go back to an earlier change, all in one window."
---

The nami app reads and edits the same files as the `nami` command, keeping your comments, so you can switch between the window, the terminal and your editor at any time. Each page below names the command it runs.

![The nami app on Discover](https://namios.dev/screens/app/discover.webp)

## Open the app

On your distribution, open **nami** from the app menu, or start it from a terminal:

```sh
nami-gui                    # the app
nami-gui --page installed   # straight to a page: discover, installed, setup, projects, history, sync
```

On a Mac, open **nami** from Applications, Launchpad or Spotlight. It shows what nami does on a Mac: Discover, Installed, Your Setup, Projects and History ([Install on a Mac](install.md#install-on-a-mac)). Discover has Mac apps from Homebrew's casks too, like Google Chrome and Visual Studio Code ([Mac apps from Homebrew's casks](install.md#mac-apps-from-homebrews-casks)).

The installer adds the app when the computer has a desktop. Don't want the app? Install with `curl -fsSL https://namios.dev/install | sh -s -- --no-app`. To remove the app: `sudo nami remove nami-gui`. Updates leave it out. To add it later: `sudo nami install nami-gui`.

There's no Apply button: what you change takes effect at once. A change for everyone on the computer asks for an administrator's password; your own changes don't.

## Find and install an app

Search **Discover** for apps and command-line tools, with apps from Flathub below. **Get** installs an app at once.

On an app's page, choose **Just me** or **System**, and pick an older release under **Version** if you need one. **In Your File** shows the lines the install writes:

```yaml file="~/user.nami"
packages:
  - gimp: "2.10"   # the same as nami install gimp@2.10
```

## Update your apps

**Installed** lists everything you have, with the updates waiting at the top.

![Installed, with the apps you have and the updates waiting](https://namios.dev/screens/app/installed.webp)

**Update All** runs `nami upgrade`, and on Linux `nami self-update` too. On a Mac, apps from Homebrew's casks are among the updates, except apps that update themselves. **Check for Updates** runs `nami update --refresh`.

## See how much space nami uses

On **Installed**, **Storage** opens how full the disk is and how much of it nami uses: your apps, apps for everyone, projects' tools, older changes kept for undo and downloads nothing uses any more (`nami storage`). Each file counts once, so the parts add up. **Largest Apps** lists what removing each app frees; click one to open its page, where **Remove** is.

**Clean Up…** says what it frees first, then removes your older changes past the newest `cleanup:` keeps (`nami gc --now --user`), then the computer's and the downloads nothing uses (`nami gc --now`). You can't undo back to the changes it removes. On a Mac it asks for an administrator's password.

## Keep your settings files

**Your Setup** is your `~/user.nami` apart from its apps: each settings file as fields, or **As Text**, and **Shell** for your `env:` and `aliases:`.

![Your Setup, with your settings files and the apps they're for](https://namios.dev/screens/app/setup.webp)

Under **Found in your home**, **Keep in nami** adopts a file such as `.bashrc` (`nami home adopt`). When an app changes a file nami keeps, choose **Keep** or **Undo** ([user.nami](user-nami.md)).

## Work on a project

**Projects** lists your folders with a `project.nami`, with their tools, services and tasks ([Projects](projects.md)).

![Projects, with a project's tools, tasks and services](https://namios.dev/screens/app/projects.webp)

**Open in Terminal** runs `nami shell`, and **Open in VS Code** opens the folder with the project's tools on its `PATH`. **Start**, **Stop** and **Logs** run `nami up -d`, `nami down` and `nami logs`. **New Project…** starts one from C, Go, Node.js, Python, Rust or an empty folder.

## Go back to an earlier change

**History** lists every change, newest first ([Changes and undo](changes.md)).

![History, with each change to go back to](https://namios.dev/screens/app/history.webp)

Click a change to see what's different. **Go Back to Here** runs `nami undo <change>`.

## Keep your setup on all your computers

On **Sync**, **Keep This Setup on All Your Computers** puts your `~/user.nami` in a private git repository, and **Sync Folders** keeps folders the same with Syncthing ([Sync](sync.md)).


## Report a problem

**Report a Problem** sends what you describe with `nami report`. **Show Report Contents** shows exactly what goes along; values marked `!secret` are always left out ([Troubleshooting](troubleshooting.md#tell-us)).

## Uninstall nami

To take nami off your computer, click **Installed**, then **Uninstall nami…** at the bottom. On a Mac, it's also in the menu bar: **nami > Uninstall nami…**. The app tells you what it removes, every app you installed with nami included, and what stays. Then it asks for your password ([Uninstall nami](install.md#uninstall-nami)). On namiOS, nami is the system itself, so there's nothing to uninstall.
