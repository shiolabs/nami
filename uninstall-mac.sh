#!/bin/sh
# Takes nami off a Mac: what install-mac.sh did, undone. (uninstall.sh is Linux's.)
#   sudo sh /nix/var/nami/uninstall.sh
#
# The store volume ("Nix Store" at /nix) goes with it when nami made it and nothing else
# uses it. When Nix shares it (installed before nami, or after), only nami's parts go:
# its state (/nix/var/nami), its roots in Nix's (/nix/var/nix/gcroots/nami), its daemon,
# its shell lines. Nix, its store and the volume stay, as they are.
set -eu
[ "$(uname -s)" = Darwin ] || { echo "uninstall-mac.sh is for macOS (uninstall.sh is Linux's)"; exit 1; }
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }
# This script is on the volume it takes away (/nix/var/nami/uninstall.sh): run from a copy,
# as sh reads it while it runs.
case $0 in
  /nix/*)
    copy=$(mktemp /tmp/nami-uninstall.XXXXXX)
    cp "$0" "$copy"
    NAMI_UNINSTALL_COPY=$copy exec sh "$copy" "$@" ;;
esac
[ -z "${NAMI_UNINSTALL_COPY:-}" ] || trap 'rm -f "$NAMI_UNINSTALL_COPY"' EXIT
cd /

LABEL="Nix Store"
STORE_PLIST=/Library/LaunchDaemons/dev.namios.store.plist
DAEMON_PLIST=/Library/LaunchDaemons/dev.namios.daemon.plist
UPDATE_PLIST=/Library/LaunchDaemons/dev.namios.update.plist

# The daily update and the daemon first: nothing writes while we clean up. (The update's
# plist only when nami made it: src/auto_update.rs.)
/bin/launchctl bootout system/dev.namios.update >/dev/null 2>&1 || true
if /usr/bin/grep -q 'Made by nami' "$UPDATE_PLIST" 2>/dev/null; then rm -f "$UPDATE_PLIST"; fi
rm -f /var/log/nami-update.log
/bin/launchctl bootout system/dev.namios.daemon >/dev/null 2>&1 || true
rm -f "$DAEMON_PLIST" /var/log/nami-daemon.log

# Shells and completions.
for rc in /etc/zshrc /etc/bashrc; do
  if [ -f "$rc" ]; then
    /usr/bin/sed -i '' '/^# >>> nami$/,/^# <<< nami$/d' "$rc"
  fi
done
rm -f /etc/paths.d/nami /usr/local/share/zsh/site-functions/_nami /usr/local/share/bash-completion/completions/nami \
  /usr/local/etc/bash_completion.d/nami /usr/local/share/fish/vendor_completions.d/nami.fish
[ -L /usr/local/bin/nami ] && rm -f /usr/local/bin/nami

# nami's app, if it's nami's (its bundle id: install-mac.sh's).
if [ -d /Applications/nami.app ] && [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' /Applications/nami.app/Contents/Info.plist 2>/dev/null)" = dev.namios.nami ]; then
  rm -rf /Applications/nami.app
  echo "removed nami's app (/Applications/nami.app)"
fi

# The apps nami copied into Applications (src/mac_apps.rs): everyone's in /Applications/nami Apps
# and each person's in ~/Applications/nami Apps, those its list (.nami-apps) names and nothing
# else. The folder goes too when nothing else is in it.
tab=$(printf '\t')
{
  echo /Applications
  /usr/bin/dscl . -list /Users NFSHomeDirectory 2>/dev/null | /usr/bin/sed -E 's/^[^[:space:]]+[[:space:]]+//' | /usr/bin/sed 's|$|/Applications|'
} | while IFS= read -r top; do
  folder="$top/nami Apps"
  [ -f "$folder/.nami-apps" ] || continue
  while IFS="$tab" read -r name _src; do
    case $name in */*|.*|'') continue ;; *.app) ;; *) continue ;; esac
    if [ -e "$folder/$name" ]; then
      chmod -R u+w "$folder/$name" 2>/dev/null || true
      if rm -rf -- "$folder/$name"; then
        echo "removed ${name%.app} from $folder"
      else
        echo "couldn't remove $folder/$name: drag it to the Trash"
      fi
    fi
  done < "$folder/.nami-apps"
  rm -f "$folder/.nami-apps"
  rmdir "$folder" 2>/dev/null || true
done

# The apps from Homebrew's casks nami keeps on the Mac's own volume (src/casks.rs): their copies in
# nami Apps went above.
if [ -d "/Library/Application Support/nami" ]; then
  chmod -R u+w "/Library/Application Support/nami" 2>/dev/null || true
  rm -rf -- "/Library/Application Support/nami"
  echo "removed the apps from Homebrew's casks nami kept (/Library/Application Support/nami)"
fi

# Whether nami made the volume (its LaunchDaemon mounts it), and whether Nix uses it.
ours=0; [ -f "$STORE_PLIST" ] && ours=1
nix=0; [ -e /nix/var/nix/db/db.sqlite ] && nix=1

if [ "$nix" = 1 ]; then
  # Nix owns the store: nami lets go of its paths (nix-collect-garbage frees them).
  rm -rf -- /nix/var/nix/gcroots/nami /nix/var/nix/gcroots/tray /nix/var/nami
  echo "released nami's store paths to Nix (nix-collect-garbage removes them); Nix and /nix stay"
  if [ "$ours" = 1 ]; then
    echo "the volume \"$LABEL\" was made by nami and is Nix's now: it stays, mounted by $STORE_PLIST"
  fi
elif [ "$ours" = 1 ]; then
  # nami's volume, nobody else's: the volume goes, and with it everything in /nix.
  uuid=$(/usr/sbin/diskutil info "$LABEL" 2>/dev/null | /usr/bin/awk -F': *' '/Volume UUID/ {print $2; exit}')
  /bin/launchctl bootout system/dev.namios.store >/dev/null 2>&1 || true
  rm -f "$STORE_PLIST"
  if [ -n "$uuid" ]; then
    /usr/sbin/diskutil unmount force "$uuid" >/dev/null 2>&1 || true
    /usr/sbin/diskutil apfs deleteVolume "$uuid" >/dev/null
    /usr/bin/security delete-generic-password -s "$uuid" /Library/Keychains/System.keychain >/dev/null 2>&1 || true
    echo "removed the volume \"$LABEL\" (/nix and everything nami installed)"
  fi
  # fstab's line for /nix (by its UUID, else the /nix one), and synthetic.conf's: /nix
  # itself is gone at the next start.
  if [ -f /etc/fstab ]; then
    /usr/bin/sed -i '' -e "/^UUID=${uuid:-none} /d" -e '\| /nix apfs rw|d' /etc/fstab
    /usr/bin/grep -Eqv '^[[:space:]]*(#|$)' /etc/fstab || rm -f /etc/fstab
  fi
  if [ -f /etc/synthetic.conf ]; then
    /usr/bin/sed -i '' -e '/^nix$/d' /etc/synthetic.conf
    /usr/bin/grep -Eqv '^[[:space:]]*(#|$)' /etc/synthetic.conf || rm -f /etc/synthetic.conf
  fi
  echo "an empty /nix stays until the Mac restarts"
else
  # A /nix nami didn't make and Nix doesn't use (made by hand): nami's own paths only.
  if [ -d /nix/var/nami/db ]; then
    n=0
    for entry in /nix/var/nami/db/*.json; do
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
  rm -rf -- /nix/var/nami
  echo "/nix itself, and anything else in it, was left in place"
fi
echo "nami removed. People may have leftovers to delete: ~/.local/state/nami, ~/.config/nami, ~/.cache/nami"
