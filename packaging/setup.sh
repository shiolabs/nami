#!/bin/sh
# Installs everything in this bundle: nami and its package catalog. Run from the unpacked bundle by the offline .run installer
# (packaging/bundle.sh, catalog included) or by the web installer
# (packaging/install-web.sh: NAMI_CHANNEL and NAMI_KEY say where the signed
# catalog is). By hand: sudo ./setup.sh
set -eu
cd "$(dirname "$0")"
# tray's self-update (an older tray, updating itself to nami) says TRAY_CHANNEL.
NAMI_CHANNEL=${NAMI_CHANNEL:-${TRAY_CHANNEL:-}}; NAMI_KEY=${NAMI_KEY:-${TRAY_KEY:-}}
[ -n "$NAMI_CHANNEL" ] || unset NAMI_CHANNEL
[ -n "$NAMI_KEY" ] || unset NAMI_KEY
# nami was tray: its places become nami's first; and nami's own folder is /nami, at
# the top (since 0.20; /nix/var/nami where / can't hold it): moved there, the old
# place left a link to it (docs/rename-migration.md), or made there on a new computer.
# (A Mac's / is read-only: before install-mac.sh has made the /nix volume, there's nothing to move.)
if [ "$(id -u)" -eq 0 ] && { [ "$(uname -s)" != Darwin ] || [ -d /nix/var ]; }; then
  mkdir -p /nix/var 2>/dev/null || true
  ./nami migrate || echo "warning: nami's files weren't moved to their places; nami moves them when it next runs as root"
fi
# A .deb or .rpm of tray's: removing it runs /opt/tray/uninstall.sh, which would
# take the store and every app with it, nami's now. It does nothing from here on.
if [ "$(id -u)" -eq 0 ] && [ -f /opt/tray/uninstall.sh ] && [ "$(pwd -P)" != /opt/tray ]; then
  printf '#!/bin/sh\n# tray is nami now: removing tray'"'"'s package leaves nami and its apps.\necho "tray is nami now (/opt/nami, /nami): nothing of it to remove here"\n' > /opt/tray/uninstall.sh
fi
# A computer nami made has nothing around nami to set up: the new nami takes
# its place (`nami self-update`), and its next daily update does the rest.
# (nami's folder: /nami, or /nix/var/nami where / can't hold it; /nix/var/nami reaches it.)
V=$(cd /nix/var/nami 2>/dev/null && pwd -P) || V=/nix/var/nami
if [ -e /run/booted-system/activate ] && [ -d $V/os ]; then
  cp ./nami $V/bin/nami.new
  chmod 755 $V/bin/nami.new
  mv $V/bin/nami.new $V/bin/nami
  ln -sfn nami $V/bin/tray
  echo "installed nami"
  exit 0
fi
cd "$(dirname "$0")"
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }
T=$V/bin/nami

if [ -e /etc/NIXOS ]; then
  # NixOS: nothing to add to your configuration. NixOS generates most of
  # /etc, but leaves alone the files it reads for local additions, and
  # those are all nami needs (nixos/nami.nix does the same declaratively):
  #   /etc/profile.local, /etc/bashrc.local, /etc/zshenv.local   PATH in shells
  #   /etc/environment.d/60-nami.conf    PATH and the app menu in graphical sessions
  #   /etc/systemd/system.control/       the daemon and the daily update (/etc/systemd/system is read-only)
  echo "NixOS: installing nami (your configuration stays as it is)"
  install -Dm755 nami "$T"
  mkdir -p $V/profiles/per-user && chmod 1777 $V/profiles/per-user
  U='$HOME/.local/state/nami/profile' P=$V/profiles/default
  block="# >>> nami
export PATH=\"$U/bin:$P/bin:$V/bin:\$PATH\"
export XDG_DATA_DIRS=\"$U/share:$P/share:\${XDG_DATA_DIRS:-/run/current-system/sw/share}\"
export XCURSOR_PATH=\"\$HOME/.icons:$U/share/icons:$P/share/icons:\${XCURSOR_PATH:-/run/current-system/sw/share/icons}\"
if [ -r \"\$HOME/.config/nami/env.sh\" ]; then . \"\$HOME/.config/nami/env.sh\"; fi
# <<< nami"
  for f in /etc/profile.local /etc/bashrc.local /etc/zshenv.local; do
    [ -f "$f" ] && sed -i '/^# >>> nami$/,/^# <<< nami$/d' "$f"
    printf '%s\n' "$block" >> "$f"
  done
  mkdir -p /etc/environment.d
  cat > /etc/environment.d/60-nami.conf <<CONF
PATH=\${HOME}/.local/state/nami/profile/bin:$P/bin:$V/bin:\${PATH}
XDG_DATA_DIRS=\${HOME}/.local/state/nami/profile/share:$P/share:\${XDG_DATA_DIRS:-/run/current-system/sw/share}
XCURSOR_PATH=\${HOME}/.icons:\${HOME}/.local/state/nami/profile/share/icons:$P/share/icons:\${XCURSOR_PATH:-/run/current-system/sw/share/icons}
CONF
  mkdir -p /etc/systemd/system.control/multi-user.target.wants
  cat > /etc/systemd/system.control/nami-daemon.service <<UNIT
[Unit]
Description=nami daemon (store access for users without root)
After=local-fs.target nix-daemon.socket
[Service]
Environment=PATH=/run/current-system/sw/bin
ExecStart=$T daemon
Restart=on-failure
[Install]
WantedBy=multi-user.target
UNIT
  ln -sf ../nami-daemon.service /etc/systemd/system.control/multi-user.target.wants/nami-daemon.service
  systemctl daemon-reload
  restart_daemon=1   # (at the end: see there)
  # The daily update, as install.sh sets it up elsewhere (its timer in system.control too).
  "$T" os-update --set-up || echo "warning: nami won't update by itself; run: sudo nami os-update --set-up"
  # An older installer left a module here to import by hand; it isn't needed.
  if [ -f /etc/nixos/nami.nix ] && ! grep -rqs --include='*.nix' --exclude=nami.nix 'nami\.nix' /etc/nixos; then
    rm -f /etc/nixos/nami.nix
  fi
else
  # (install.sh leaves restarting the daemon to the end of this script; install-mac.sh
  # restarts a Mac's, and says what it does to /nix: a volume of its own, the first time.)
  if [ "$(uname -s)" = Darwin ]; then
    sh ./install-mac.sh ./nami
  else
    NAMI_SETUP_RESTARTS=1 ./install.sh ./nami >/dev/null
    restart_daemon=1
  fi
  # (Where install.sh put it: on a new computer, nami's folder is made there. A Mac's is
  # /nix/var/nami: its / is read-only.)
  V=$(cd /nix/var/nami && pwd -P); T=$V/bin/nami
  echo "installed nami ($T)"
fi

# The uninstaller, for sudo nami self-uninstall (and the web uninstaller): $V/uninstall.sh (a Mac's own, in /nix/var/nami)
if [ "$(uname -s)" = Darwin ]; then install -m755 uninstall-mac.sh $V/uninstall.sh; else install -m755 uninstall.sh $V/uninstall.sh; fi

if [ -d catalog ]; then
  # The offline installer (.run) carries the catalog.
  echo "installing the package catalog ($(du -sh catalog | cut -f1))"
  rm -rf $V/catalog && mkdir -p $V/catalog
  cp -r catalog/. $V/catalog/
  icons=""
  [ -e catalog/icons-url ] && icons="--icons $(cat catalog/icons-url)"   # app icons from a server
  "$T" update --from $V/catalog/index-x86_64-linux.json.xz --drvs $V/catalog/drvs --unsigned $icons
else
  # Online: a signed channel (NAMI_CHANNEL, signed by NAMI_KEY).
  : "${NAMI_CHANNEL:?no catalog in this bundle, and no NAMI_CHANNEL to download one from}"
  echo "getting the package catalog from $NAMI_CHANNEL"
  "$T" update --from "$NAMI_CHANNEL" ${NAMI_KEY:+--trust "$NAMI_KEY"}
fi

# nami's window, while the computer's file lists it (the installer's
# `nami install nami-gui`): its recipe as this nami has it, so an update brings
# the window's version that goes with it, not only the command's. Once it's
# taken out (`sudo nami remove nami-gui`), an update leaves it out.
if [ -d /etc/nami/packages/nami-gui ] &&
   grep -qE '^[[:space:]]*-[[:space:]]*["'"'"']?(\./packages/|/etc/nami/packages/)?nami-gui["'"'"']?[[:space:]]*(#.*)?$' /etc/nami/system.nami 2>/dev/null; then
  "$T" install nami-gui >/dev/null 2>&1 && echo "nami's window updated" || echo "nami's window: not updated (sudo nami install nami-gui tries again)"
fi

# The daemon, restarted onto this nami last of all. Run by the daemon itself
# (`nami self-update` as someone without root), restarting it now would end this
# script, and the update's answer, midway: so a few seconds after this ends, as a
# job of its own.
if [ -n "${restart_daemon:-}" ] && [ -d /run/systemd/system ]; then
  if grep -q nami-daemon.service /proc/self/cgroup 2>/dev/null; then
    systemd-run --quiet --collect --on-active=5 --unit="nami-daemon-restart-$$" systemctl restart nami-daemon.service
  else
    systemctl restart nami-daemon.service
  fi
fi

echo
if [ "$(uname -s)" = Darwin ]; then echo "Done. Open a new terminal, then: nami --help"; else echo "Done. Log out and in once, then: nami --help"; fi
if [ -e /etc/NIXOS ]; then
  echo "Nothing to add to your NixOS configuration."
fi
