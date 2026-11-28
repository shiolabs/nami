#!/bin/sh
# Removes nami from this machine: the store paths nami registered (and only
# those: other things in /nix/store, e.g. from Nix, are left alone), its state,
# and the files install.sh added to /etc.
# usage: sudo ./uninstall.sh
set -eu
# (A Mac's: uninstall-mac.sh, beside it.)
if [ "$(uname -s)" = Darwin ]; then exec sh "$(dirname "$0")/uninstall-mac.sh" "$@"; fi
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }

# nami's folder: /nami (since 0.20), or /nix/var/nami where / couldn't hold it.
# /nix/var/nami reaches it either way (the folder, or a link to /nami).
top=/nami var=/nix/var/nami tray=/nix/var/tray
if V=$(cd "$var" 2>/dev/null && pwd -P); then :; elif [ -x "$top/bin/nami" ]; then V=$top; else V=$var; fi

# The services nami set up, and its /etc files: gone (one you changed
# stays), and what they replaced back.
if [ -x $V/bin/nami ]; then
  $V/bin/nami let-go || echo "(nami couldn't take back its services and /etc files; they're left as they are)"
fi

# The daily update (install.sh's), and the daemon, first: nothing writes while we clean up.
for u in /etc/systemd/system /etc/systemd/system.control; do  # (system.control: NixOS's)
  if grep -q '^# Made by nami' $u/nami-update.timer 2>/dev/null; then
    systemctl stop nami-update.timer nami-update.service 2>/dev/null || true
    rm -f $u/nami-update.timer $u/nami-update.service $u/timers.target.wants/nami-update.timer
  fi
done
if [ -f /etc/systemd/system/nami-daemon.service ]; then
  systemctl disable --now nami-daemon.service 2>/dev/null || true
  rm -f /etc/systemd/system/nami-daemon.service
  systemctl daemon-reload 2>/dev/null || true
fi
if [ -f /etc/systemd/system.control/nami-daemon.service ]; then  # NixOS
  systemctl stop nami-daemon.service 2>/dev/null || true
  rm -f /etc/systemd/system.control/nami-daemon.service /etc/systemd/system.control/multi-user.target.wants/nami-daemon.service
  systemctl daemon-reload 2>/dev/null || true
fi
if [ -f /etc/init.d/nami-daemon ]; then
  rc-service nami-daemon stop 2>/dev/null || true
  rc-update del nami-daemon default 2>/dev/null || true
  rm -f /etc/init.d/nami-daemon
fi
if [ -d /etc/sv/nami-daemon ]; then
  rm -f /var/service/nami-daemon
  rm -rf /etc/sv/nami-daemon
fi

db=$V/db
if [ -e /nix/var/nix/db/db.sqlite ]; then
  # Nix owns the store: let go of nami's paths (nix-collect-garbage frees them).
  rm -rf -- /nix/var/nix/gcroots/tray /nix/var/nix/gcroots/nami
  echo "released nami's store paths to Nix (nix-collect-garbage removes them)"
elif [ -d "$db" ]; then
  n=0
  for entry in "$db"/*.json; do
    [ -e "$entry" ] || continue
    path=/nix/store/$(basename "$entry" .json)
    case "$path" in /nix/store/?*) ;; *) continue ;; esac
    if [ -e "$path" ] || [ -L "$path" ]; then
      chmod -R u+w "$path" 2>/dev/null || true
      rm -rf -- "$path"
    fi
    n=$((n + 1))
  done
  echo "removed $n store paths"
fi
# nami's folder, and its other names: /nix/var/nami, a link to /nami (or /nami, a link to
# /nix/var/nami where it stayed there); tray's, a link since nami moved it, or tray's
# own if it never was. (A /nami of someone's own, not nami's, stays.)
case $V in "$top"|"$var") rm -rf -- "$V" ;; esac
[ -L "$var" ] && rm -f -- "$var"
[ -L "$top" ] && case $(readlink "$top") in nix/var/nami|/nix/var/nami) rm -f -- "$top" ;; esac
if [ -L "$tray" ]; then rm -f -- "$tray"; else rm -rf -- "$tray"; fi
# (end of nami's folder)
case $(readlink /run/opengl-driver 2>/dev/null) in /nami/profiles/runtime|/nix/var/nami/profiles/runtime) rm -f /run/opengl-driver ;; esac
# nami's app menu entries (marked) and the app manager launcher
for f in /usr/local/share/applications/*.desktop; do
  [ -f "$f" ] && grep -q '^X-\(Nami\|Tray\)-Managed=true' "$f" && rm -f -- "$f"
done
rm -f /usr/local/share/applications/nami.desktop /usr/local/share/icons/hicolor/scalable/apps/nami.svg
for f in /etc/profile.local /etc/bashrc.local /etc/zshenv.local; do
  [ -f "$f" ] && sed -i '/^# >>> nami$/,/^# <<< nami$/d' "$f"
done
for f in /etc/profile.d/nami.sh /etc/environment.d/50-nami.conf /etc/environment.d/60-nami.conf /etc/fish/conf.d/nami.fish \
         /etc/systemd/user-environment-generators/50-nami /usr/share/polkit-1/actions/org.namios.change-computer.policy \
         /etc/tmpfiles.d/nami.conf /etc/local.d/nami.start /usr/share/polkit-1/actions/org.trayos.change-computer.policy \
         /etc/profile.d/tray.sh /etc/fish/conf.d/tray.fish /etc/systemd/user-environment-generators/50-tray /etc/tmpfiles.d/tray.conf \
         /usr/local/share/bash-completion/completions/nami /usr/local/share/zsh/site-functions/_nami /etc/fish/completions/nami.fish; do
  rm -f -- "$f"
done
[ -L /usr/local/bin/nami ] && rm -f /usr/local/bin/nami
[ -L /usr/local/bin/tray ] && rm -f /usr/local/bin/tray
for rc in /etc/bash.bashrc /etc/bashrc; do
  [ -f "$rc" ] && [ ! -L "$rc" ] && sed -i '/nami\/env.sh.*# nami$/d; /tray\/env.sh.*# tray$/d' "$rc"
done
echo "nami removed (/nix/store itself and anything else in it were left in place)"
if [ -d /etc/nami ]; then
  # (/etc/nami is often a link to the computer's folder in /etc/nami-config.)
  if [ -d /etc/nami-config ]; then
    echo "your computer's nami files are still in /etc/nami and /etc/nami-config (to set it up again); sudo rm -r /etc/nami /etc/nami-config removes them"
  else
    echo "your computer's nami files are still in /etc/nami (to set it up again); sudo rm -r /etc/nami removes them"
  fi
fi
if [ -e /etc/NIXOS ] && [ -e /etc/nixos/nami.nix ]; then
  echo "NixOS: if your configuration imports nami.nix, remove that import and"
  echo "/etc/nixos/nami.nix, then run: sudo nixos-rebuild switch"
fi
echo "users may have leftovers to delete: ~/.local/state/nami, and app menu entries"
echo "in ~/.local/share/applications marked X-Nami-Managed=true"
