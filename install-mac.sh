#!/bin/sh
# nami on a Mac (Apple silicon or Intel): what install.sh is on Linux. Run as root, by
# setup.sh (the web installer's, and `nami self-update`'s) or by hand:
#   sudo ./install-mac.sh [path-to-nami-binary]
#
# Packages' programs name their files under /nix/store, so /nix has to be there. macOS
# keeps / read-only, so /nix is a volume of its own, as the Nix installers make it:
#   /etc/synthetic.conf  `nix`: macOS makes an empty /nix at the top at each start
#   an APFS volume       "Nix Store" in the startup disk's container (sharing its space),
#                        encrypted when FileVault is on (its password in the System
#                        keychain, which unlocks it as the computer starts)
#   /etc/fstab           UUID=<it> /nix apfs rw,noauto,nobrowse,nosuid,noatime,owners
#                        (nobrowse: not in Finder or on the desktop)
#   a LaunchDaemon       dev.namios.store mounts it at each start (and unlocks it)
# The volume is case-insensitive APFS, as Nix makes it; nami unpacks names that differ
# only in case as Nix does on macOS (`~nix~case~hack~`), so the two can share a store.
#
# A Mac with Nix on it already (the official installer's, Determinate's): its /nix is
# used as it is, and Nix keeps its store (nami goes through nix-store, as on Linux).
# A Mac with nami already: nothing of the volume is done again (self-update runs this).
#
# Then: the daemon (dev.namios.daemon: it fetches into the store for everyone, so nobody
# needs sudo to install), the daily update (dev.namios.update: `nami os-update` once a day,
# unless /etc/nami/system.nami says `updates: manual`), shells (/etc/zshrc, /etc/bashrc,
# /etc/paths.d) and completions.
# And nami's app, /Applications/nami.app, when it's asked for (NAMI_APP=1) or there already.
# uninstall-mac.sh takes all of it back.
set -eu

bin=${1:-$(dirname "$0")/target/release/nami}
[ "$(uname -s)" = Darwin ] || { echo "install-mac.sh is for macOS (install.sh is Linux's)"; exit 1; }
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }
[ -x "$bin" ] || { echo "no nami binary at $bin"; exit 1; }

LABEL="Nix Store"
STORE_PLIST=/Library/LaunchDaemons/dev.namios.store.plist
DAEMON_PLIST=/Library/LaunchDaemons/dev.namios.daemon.plist
APFS_UTIL=/System/Library/Filesystems/apfs.fs/Contents/Resources/apfs.util
say() { printf '%s\n' "$*"; }

# ---------- /nix ----------

# Whether /nix is a volume of its own, mounted (not the empty folder synthetic.conf makes).
nix_mounted() { /sbin/mount | /usr/bin/grep -q ' on /nix (' ; }

# The "Nix Store" volume's UUID, if there is one.
volume_uuid() {
  /usr/sbin/diskutil info "$LABEL" 2>/dev/null | /usr/bin/awk -F': *' '/Volume UUID/ {print $2; exit}'
}

make_nix() {
  # 1. /nix at the top: synthetic.conf says so; apfs.util makes it now, not at the next start.
  if [ -L /nix ]; then
    say "/nix is a symbolic link ($(readlink /nix)); nami needs /nix itself. Remove the link (and its line in /etc/synthetic.conf), restart, and run this again."
    exit 1
  fi
  if ! /usr/bin/grep -qE '^nix($|[[:space:]])' /etc/synthetic.conf 2>/dev/null; then
    printf 'nix\n' >> /etc/synthetic.conf
    chmod 644 /etc/synthetic.conf
    say "added nix to /etc/synthetic.conf"
  fi
  if [ ! -d /nix ]; then
    { "$APFS_UTIL" -t || true; "$APFS_UTIL" -B || true; } >/dev/null 2>&1
  fi
  i=0; while [ ! -d /nix ] && [ $i -lt 50 ]; do sleep 0.2; i=$((i + 1)); done
  [ -d /nix ] || { say "macOS didn't make /nix (from /etc/synthetic.conf); restart the Mac, then run this again"; exit 1; }

  # 2. The volume: the one there is (from before), else a new one in the startup disk's container.
  uuid=$(volume_uuid)
  if [ -z "$uuid" ]; then
    disk=$(/usr/bin/stat -f '%Sd' /); disk=${disk%s[0-9]*}
    say "making the APFS volume \"$LABEL\" on $disk"
    /usr/sbin/diskutil apfs addVolume "$disk" APFS "$LABEL" -nomount >/dev/null
    i=0; while [ -z "$(volume_uuid)" ] && [ $i -lt 50 ]; do sleep 0.2; i=$((i + 1)); done
    uuid=$(volume_uuid)
    [ -n "$uuid" ] || { say "the volume \"$LABEL\" was made, but macOS doesn't say its UUID"; exit 1; }
    new=1
  else
    say "using the APFS volume \"$LABEL\" that's here ($uuid)"
    new=0
  fi
  /usr/sbin/diskutil unmount force "$uuid" >/dev/null 2>&1 || true

  # 3. fstab: how it's mounted (before it ever is), hidden from Finder.
  if ! /usr/bin/grep -q ' /nix apfs ' /etc/fstab 2>/dev/null; then
    printf 'UUID=%s /nix apfs rw,noauto,nobrowse,nosuid,noatime,owners\n' "$uuid" >> /etc/fstab
    chmod 644 /etc/fstab
  fi

  # 4. Encrypted when FileVault is on: a random password, kept in the System keychain,
  # where only the mounting (and macOS's own unlocking) reads it.
  encrypted=0
  if /usr/sbin/diskutil apfs listCryptoUsers "$uuid" 2>/dev/null | /usr/bin/grep -q 'Type: Disk'; then
    encrypted=1
  elif [ "$new" = 1 ] && /usr/bin/fdesetup isactive >/dev/null 2>&1; then
    say "FileVault is on: encrypting the volume"
    /usr/sbin/diskutil mount "$uuid" >/dev/null
    password=$(/usr/bin/xxd -l 32 -p -c 256 /dev/random)
    /usr/bin/security add-generic-password -a "$LABEL" -s "$uuid" -l "$LABEL encryption password" -D "Encrypted volume password" \
      -j "Added by nami's installer for $STORE_PLIST" -w "$password" \
      -T /System/Library/CoreServices/APFSUserAgent -T /System/Library/CoreServices/CSUserAgent -T /usr/bin/security \
      /Library/Keychains/System.keychain
    printf '%s' "$password" | /usr/sbin/diskutil apfs encryptVolume "$uuid" -user disk -stdinpassphrase >/dev/null
    unset password
    /usr/sbin/diskutil unmount force "$uuid" >/dev/null 2>&1 || true
    encrypted=1
  fi

  # 5. Mounted at each start, by a LaunchDaemon (unlocked first when it's encrypted).
  if [ "$encrypted" = 1 ]; then
    mount_args="<string>/bin/sh</string><string>-c</string><string>/usr/bin/security find-generic-password -s '$uuid' -w | /usr/sbin/diskutil apfs unlockVolume '$uuid' -mountpoint /nix -stdinpassphrase</string>"
  else
    mount_args="<string>/usr/sbin/diskutil</string><string>mount</string><string>-mountPoint</string><string>/nix</string><string>$uuid</string>"
  fi
  cat > "$STORE_PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Made by nami (install-mac.sh): mounts the "$LABEL" volume ($uuid) at /nix. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.namios.store</string>
  <key>RunAtLoad</key>
  <true/>
  <key>ProgramArguments</key>
  <array>$mount_args</array>
</dict>
</plist>
EOF
  chmod 644 "$STORE_PLIST"
  /bin/launchctl bootout system/dev.namios.store >/dev/null 2>&1 || true
  /bin/launchctl bootstrap system "$STORE_PLIST" 2>/dev/null || true
  /bin/launchctl kickstart -k system/dev.namios.store >/dev/null 2>&1 || true
  i=0; while ! nix_mounted && [ $i -lt 150 ]; do sleep 0.2; i=$((i + 1)); done
  nix_mounted || { say "the volume \"$LABEL\" didn't mount at /nix (see: sudo launchctl print system/dev.namios.store)"; exit 1; }
  # Files with owners other than root (the daemon's, a person's profile folder).
  if /usr/sbin/diskutil info /nix | /usr/bin/grep -q 'Owners:.*Disabled'; then
    /usr/sbin/diskutil enableOwnership /nix >/dev/null
  fi
  # Not indexed by Spotlight: a store is a great many files nobody searches.
  /usr/bin/mdutil -i off /nix >/dev/null 2>&1 || true
  say "/nix is the volume \"$LABEL\"$([ "$encrypted" = 1 ] && echo ', encrypted')"
}

if [ -e /nix/var/nix/db/db.sqlite ]; then
  say "Nix is on this Mac: nami shares its store (/nix stays Nix's)"
elif nix_mounted || [ -d /nix/var/nami ]; then
  : # nami's, from before
else
  make_nix
fi

# ---------- nami ----------

mkdir -p /nix/store /nix/var/nami/bin /nix/var/nami/profiles/per-user
chmod 1777 /nix/var/nami/profiles/per-user   # users make their own profile folder, like /tmp
cp "$bin" /nix/var/nami/bin/nami.new && chmod 755 /nix/var/nami/bin/nami.new && mv -f /nix/var/nami/bin/nami.new /nix/var/nami/bin/nami
# (What macOS says of a program downloaded with a browser: nami came with curl, but a copy
# made by hand may carry it.)
/usr/bin/xattr -d com.apple.quarantine /nix/var/nami/bin/nami 2>/dev/null || true
mkdir -p /usr/local/bin
ln -sf /nix/var/nami/bin/nami /usr/local/bin/nami

# ---------- the daemon ----------

# It waits for /nix (mounted by dev.namios.store) before it starts.
cat > "$DAEMON_PLIST" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Made by nami (install-mac.sh): installs into the store for people without sudo. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.namios.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>/bin/wait4path /nix/var/nami/bin/nami &amp;&amp; exec /nix/var/nami/bin/nami daemon</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>/var/log/nami-daemon.log</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/nix/var/nami/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
</dict>
</plist>
EOF
chmod 644 "$DAEMON_PLIST"
# (Booting the daemon out ends whatever it runs. Nothing that runs this script goes through
# it: a Mac's self-update and os-update are sudo's or launchd's own, never the daemon's, as
# profile::is_admin_user has no admins on a Mac. Keep it so.)
/bin/launchctl bootout system/dev.namios.daemon >/dev/null 2>&1 || true
/bin/launchctl bootstrap system "$DAEMON_PLIST" 2>/dev/null || /bin/launchctl kickstart -k system/dev.namios.daemon
i=0; while [ ! -S /nix/var/nami/daemon.sock ] && [ $i -lt 100 ]; do sleep 0.2; i=$((i + 1)); done
[ -S /nix/var/nami/daemon.sock ] || say "warning: the nami daemon didn't start (see /var/log/nami-daemon.log); people need sudo for nami until it does"

# ---------- the daily update ----------

# /Library/LaunchDaemons/dev.namios.update.plist, made by nami itself (src/auto_update.rs), as
# system.nami's `updates:` says: launchd runs `nami os-update` once a day (its log:
# /var/log/nami-update.log). From a self-update too, so a Mac updating to this nami gets it.
/nix/var/nami/bin/nami os-update --set-up || say "warning: nami won't update by itself; run: sudo nami os-update --set-up"

# ---------- shells ----------

# Everyone's PATH: theirs first (nami install without sudo), the computer's, nami itself.
# Read by the blocks below, after macOS's path_helper (/etc/zprofile, /etc/profile) has
# put /etc/paths first, so nami's programs come before the system's, as they should.
cat > /nix/var/nami/nami.sh <<'EOF'
# Made by nami (install-mac.sh): its programs on PATH, for zsh and bash.
# (Each first, also when path_helper put one last, from /etc/paths.d.)
for _nami_d in /nix/var/nami/bin /nix/var/nami/profiles/default/bin "$HOME/.local/state/nami/profile/bin"; do
  _nami_p=":$PATH:"
  while case $_nami_p in *":$_nami_d:"*) true ;; *) false ;; esac; do
    _nami_p="${_nami_p%%:"$_nami_d":*}:${_nami_p#*:"$_nami_d":}"
  done
  _nami_p=${_nami_p#:}; _nami_p=${_nami_p%:}
  PATH="$_nami_d${_nami_p:+:$_nami_p}"
done
unset _nami_d _nami_p
export PATH
export MANPATH="$HOME/.local/state/nami/profile/share/man:/nix/var/nami/profiles/default/share/man:${MANPATH:-}"
# env: and aliases: from your user.nami
if [ -r "$HOME/.config/nami/env.sh" ]; then . "$HOME/.config/nami/env.sh"; fi
EOF
chmod 644 /nix/var/nami/nami.sh
# The guarded block (a macOS update may put back its own /etc/zshrc: `sudo nami self-update`
# or the installer adds it again).
block='# >>> nami
if [ -e /nix/var/nami/nami.sh ]; then . /nix/var/nami/nami.sh; fi
# <<< nami'
for rc in /etc/zshrc /etc/bashrc; do
  if [ -f "$rc" ]; then
    /usr/bin/sed -i '' '/^# >>> nami$/,/^# <<< nami$/d' "$rc"
  fi
  printf '\n%s\n' "$block" >> "$rc"
done
# Shells and apps that don't read those still find nami (path_helper reads /etc/paths.d).
printf '/nix/var/nami/bin\n' > /etc/paths.d/nami

# Tab completion: zsh's site-functions (in macOS zsh's fpath, for compinit), and where
# bash-completion looks (Homebrew's, and its own).
mkdir -p /usr/local/share/zsh/site-functions /usr/local/share/bash-completion/completions /usr/local/etc/bash_completion.d
/nix/var/nami/bin/nami completions zsh > /usr/local/share/zsh/site-functions/_nami
/nix/var/nami/bin/nami completions bash > /usr/local/share/bash-completion/completions/nami
ln -sf /usr/local/share/bash-completion/completions/nami /usr/local/etc/bash_completion.d/nami
if [ -d /usr/local/share/fish/vendor_completions.d ] || command -v fish >/dev/null 2>&1; then
  mkdir -p /usr/local/share/fish/vendor_completions.d
  /nix/var/nami/bin/nami completions fish > /usr/local/share/fish/vendor_completions.d/nami.fish
fi

# ---------- nami's app ----------

# nami.app (nami's window), when this release has it beside nami: copied to /Applications
# when it's asked for (NAMI_APP=1, the web installer's unless --no-app), and brought up to date
# where it's there already (self-update). Moved to the Trash, it stays out. An app of that name
# that isn't nami's (another bundle id) is left alone.
app_src=$(dirname "$bin")/nami.app
app_dst=/Applications/nami.app
bundle_id() { /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$1/Contents/Info.plist" 2>/dev/null || true; }
if [ -d "$app_src" ]; then
  if { [ "${NAMI_APP:-}" = 1 ] && [ ! -e "$app_dst" ]; } || { [ -d "$app_dst" ] && [ "$(bundle_id "$app_dst")" = dev.namios.nami ]; }; then
    rm -rf "$app_dst.new"
    /usr/bin/ditto "$app_src" "$app_dst.new"
    # root's, as Applications' own apps are: not whoever unpacked the release (an account
    # that isn't an administrator could change it otherwise).
    chown -R root:wheel "$app_dst.new"
    /usr/bin/xattr -dr com.apple.quarantine "$app_dst.new" 2>/dev/null || true
    rm -rf "$app_dst"
    mv "$app_dst.new" "$app_dst"
    say "nami's app is in Applications (nami.app)"
  fi
fi

echo "nami installed. Open a new terminal, then: nami install ripgrep"
