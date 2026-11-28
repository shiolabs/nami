---
title: "Apps from other distributions"
description: "Put an AppImage in your app menu, and turn a .deb into a package on any distribution."
---


## Put an AppImage in the launcher

An AppImage runs as it is.
To add it to your app menu with its own name and icon:

```sh
nami add-to-launcher ~/Downloads/Obsidian-1.7.7.AppImage
```

```ini file="~/.local/share/applications/nami-obsidian.desktop"
[Desktop Entry]
Type=Application
Name=Obsidian
Comment=Obsidian
Categories=Office;
Exec=/nami/bin/nami start /home/ada/Downloads/Obsidian-1.7.7.AppImage %U
Icon=/home/ada/.local/share/nami/icons/nami-obsidian.png
Path=/home/ada/Downloads
```


## Install an app that's only a .deb

Some apps are only in Ubuntu's archive or on their maker's site as a `.deb`. `nami recipe from-deb` turns one into a package you can install on any distribution.

```sh
nami recipe from-deb dctrl-tools
nami build ./dctrl-tools
nami install ./dctrl-tools
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - ./dctrl-tools
```

[Your own packages](recipes.md#package-a-deb) explains what it writes and how to fix what it couldn't match.

