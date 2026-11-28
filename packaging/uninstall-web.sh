#!/bin/sh
# nami's web uninstaller: curl -fsSL @BASE@/uninstall | sh
# Takes nami itself off this computer with the uninstaller it came with (uninstall.sh in
# nami's folder: /nami or /nix/var/nami; a Mac's, /nix/var/nami), as `sudo nami
# self-uninstall` does, for when nami won't run or isn't on PATH. It says what it removes and
# what stays, and asks you to type `uninstall` first (on the terminal: stdin is this script).
#
# Options: curl -fsSL @BASE@/uninstall | sh -s -- --yes
#   --yes   don't ask (for scripts)
set -eu
BASE="@BASE@"
yes=""
while [ $# -gt 0 ]; do
  case $1 in
    -y|--yes) yes=1 ;;
    -h|--help) printf '%s\n' "nami's uninstaller: uninstalls nami itself from this computer." "  --yes   don't ask (for scripts)"; exit 0 ;;
    -*) echo "unknown option: $1 (try --help)"; exit 2 ;;
    *) echo "This uninstalls nami itself. To remove $*, run: nami remove $*"; exit 2 ;;
  esac
  shift
done

# Colours only on a terminal.
if [ -t 1 ] && [ "${NO_COLOR:-}" = "" ]; then
  B=$(printf '\033[1m') D=$(printf '\033[2m') Y=$(printf '\033[33;1m') N=$(printf '\033[0m')
else
  B="" D="" Y="" N=""
fi

mac=""; [ "$(uname -s)" = Darwin ] && mac=1
computer=computer; [ -z "$mac" ] || computer=Mac
# namiOS: nami is the system there.
if [ -z "$mac" ] && { [ -e /etc/namios ] || { [ -e /run/booted-system/activate ] && [ -d /nix/var/nami/os ]; }; }; then
  echo "On namiOS, nami is the system itself, so it can't be uninstalled."
  exit 1
fi
# nami's uninstaller: in its folder (/nix/var/nami reaches it, the folder or a link to /nami).
script=""
for s in /nix/var/nami/uninstall.sh /nami/uninstall.sh; do
  [ -f "$s" ] && { script=$s; break; }
done
# nami from a .deb or .rpm: removed by the package manager (its removal runs uninstall.sh).
package=""
if [ -z "$mac" ] && [ -f /opt/nami/uninstall.sh ]; then
  if dpkg-query -W -f='${Status}' nami 2>/dev/null | grep -q 'install ok installed'; then package="dpkg --remove nami"
  elif rpm -q nami >/dev/null 2>&1; then package="rpm -e nami"
  fi
fi
if [ -z "$script$package" ]; then
  if [ -e /nix/var/nami ] || [ -e /nami/bin/nami ] || [ -L /usr/local/bin/nami ]; then
    echo "nami's uninstaller isn't on this computer (/nix/var/nami/uninstall.sh). Install nami again ($BASE/install), then uninstall it."
    exit 1
  fi
  echo "nami isn't installed on this $computer: there's nothing to remove."
  exit 0
fi
if [ "$(id -u)" -eq 0 ]; then SUDO=""
elif command -v sudo >/dev/null 2>&1; then SUDO=sudo
else echo "Run this as root (there's no sudo)."; exit 1
fi

if [ -z "$yes" ]; then
  nix=""; [ -e /nix/var/nix/db/db.sqlite ] && nix=1
  own=""; [ -f /Library/LaunchDaemons/dev.namios.store.plist ] && own=1
  if [ -n "$nix" ]; then store="its part of the store (Nix keeps /nix and its own)"
  elif [ -n "$mac" ] && [ -n "$own" ]; then store="its store (the Nix Store volume, /nix)"
  else store="its store (/nix/store)"
  fi
  apps="  - every app and tool installed with nami, for everyone on this $computer"
  [ -z "$nix" ] || apps="$apps (Nix's nix-collect-garbage frees them)"
  echo
  echo "${Y}warning:${N} ${B}This uninstalls nami itself from this $computer, not a package: nami, every app and tool installed with it, and $store.${N}"
  echo
  echo "It removes:"
  if [ -n "$mac" ]; then
    echo "  - nami's command, the nami app in Applications, and nami's background service"
    echo "$apps"
    echo "  - the apps nami put in Applications, in the nami Apps folders (what's yours there stays)"
    echo "  - nami's lines in the shells' startup files, and its Tab completion"
  else
    echo "  - nami's command, the nami app, and nami's background service and daily update"
    echo "$apps"
    echo "  - the services and /etc files nami set up (files they replaced come back)"
    echo "  - nami's lines in the shells' startup files, its app menu entries and Tab completion"
  fi
  echo "It keeps:"
  if [ -n "$mac" ]; then
    echo "  - your nami files: ~/user.nami and ~/.config/nami"
  else
    echo "  - your nami files: ~/user.nami, ~/.config/nami, and the system's in /etc/nami"
  fi
  echo "  - your home folder and everything in it"
  [ -n "$mac" ] && [ -n "$own" ] && [ -z "$nix" ] || echo "  - anything in /nix/store that isn't nami's"
  echo
  echo "To remove one app instead, use nami remove <name>."
  echo
  printf 'Type %suninstall%s to uninstall nami, or press Return to cancel: ' "$B" "$N"
  # (stdin is this script, from curl: the answer comes from the terminal.)
  answer=""
  if ! { read -r answer < /dev/tty; } 2>/dev/null; then
    echo
    echo "There's no terminal to ask on: run it with --yes to uninstall without asking (curl -fsSL $BASE/uninstall | sh -s -- --yes)."
  fi
  case $(printf '%s' "$answer" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]') in
    uninstall) ;;
    *) echo "Nothing was removed."; exit 1 ;;
  esac
fi

if [ -n "$SUDO" ]; then
  echo "Uninstalling needs your password (sudo):"
  # (sudo asks on the terminal itself.)
  $SUDO -v || { echo "Nothing was removed: sudo didn't work."; exit 1; }
fi
echo "Uninstalling nami itself from this $computer."
cd /
# (Not on this script's stdin: what's left of it is sh's to read.)
if [ -n "$package" ]; then
  echo "nami came as a package: removing it with ${package%% *}."
  $SUDO $package < /dev/null
else
  $SUDO sh "$script" < /dev/null
fi
echo "${D}To install nami again: curl -fsSL $BASE/install | sh${N}"
