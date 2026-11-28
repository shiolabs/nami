#!/bin/sh
# nami installer. Works on any Linux: needs only sh and root once.
# usage: sudo ./install.sh [path-to-nami-binary]
# (On a Mac: install-mac.sh, beside it.)
set -eu
if [ "$(uname -s)" = Darwin ]; then exec sh "$(dirname "$0")/install-mac.sh" "$@"; fi

bin=${1:-$(dirname "$0")/target/release/nami}
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }
[ -x "$bin" ] || { echo "no nami binary at $bin"; exit 1; }

# Immutable distros (Silverblue, Kinoite, SteamOS...) have a read-only /,
# so keep the data in /var and bind-mount it to /nix.
if ! mkdir -p /nix 2>/dev/null; then
  if command -v systemctl >/dev/null; then
    # (A computer that had tray keeps its store where it is.)
    nixdir=/var/lib/nami/nix; [ -d /var/lib/tray/nix ] && nixdir=/var/lib/tray/nix
    mkdir -p $nixdir
    cat > /etc/systemd/system/nix.mount <<EOF
[Unit]
Description=nami store (/nix)
[Mount]
What=$nixdir
Where=/nix
Type=none
Options=bind
[Install]
WantedBy=local-fs.target
EOF
    chattr -i / 2>/dev/null || true; mkdir -p /nix; chattr +i / 2>/dev/null || true
    systemctl daemon-reload && systemctl enable --now nix.mount
  else
    echo "/ is read-only and there is no systemd to bind-mount /nix"; exit 1
  fi
fi

# nami was tray: its places (/nix/var/tray, /etc/tray, ...) become nami's, the old
# names left pointing there (docs/rename-migration.md). Nothing to do on a new computer.
# A computer that had tray keeps tray's names for nami until 1.0; a new one never has them.
had_tray=""
if [ -e /nix/var/tray ] || [ -L /nix/var/tray ] || [ -e /etc/tray ] || [ -e /usr/local/bin/tray ] || [ -L /usr/local/bin/tray ]; then
  had_tray=1
fi
# nami's own folder (its programs, catalog, profiles): /nami, at the top. Where / can't
# hold it (read-only, or not on the file system /nix is on: an ephemeral /, a /nix of its
# own), /nix/var/nami, as before 0.20. nami decides, and moves an older install's folder
# there, /nix/var/nami left a link to it (`nami migrate`; docs/rename-migration.md).
mkdir -p /nix/store /nix/var
"$bin" migrate || echo "warning: nami's folder wasn't made or moved; nami does it when it next runs as root"
[ -d /nix/var/nami ] || mkdir -p /nix/var/nami
# (Where it is: /nami, or /nix/var/nami. /nix/var/nami reaches it either way.)
V=$(cd /nix/var/nami && pwd -P)
mkdir -p $V/bin $V/profiles/per-user
chmod 1777 $V/profiles/per-user   # users create their own profile dir, like /tmp
install -m 755 "$bin" $V/bin/nami
ln -sf $V/bin/nami /usr/local/bin/nami 2>/dev/null || true
# `tray` is nami too, until 1.0, where tray was: scripts and shells that say tray keep working.
if [ -n "$had_tray" ]; then
  ln -sfn nami $V/bin/tray
  ln -sf $V/bin/tray /usr/local/bin/tray 2>/dev/null || true
fi
# What tray's installer put around it, each now nami's (below): never both.
rm -f /etc/profile.d/tray.sh /etc/fish/conf.d/tray.fish /etc/systemd/user-environment-generators/50-tray \
  /etc/environment.d/50-tray.conf /etc/tmpfiles.d/tray.conf /etc/local.d/tray.start

# SELinux (Fedora/RHEL): label the store like /usr so confined services can run it.
if command -v semanage >/dev/null && command -v restorecon >/dev/null; then
  semanage fcontext -a -t usr_t '/nix/store(/.*)?' 2>/dev/null || true
  # and nami itself as a program, so systemd may start its daemon (default_t: it can't)
  semanage fcontext -a -t bin_t "$V/bin(/.*)?" 2>/dev/null || true
  restorecon -R /nix "$V" 2>/dev/null || true
fi

P=$V/profiles/default
R=$V/profiles/runtime
# Each user's own profile (nami install without sudo) comes first.
U='$HOME/.local/state/nami/profile'
# Only nixpkgs programs read LOCALE_ARCHIVE (a nixpkgs glibc patch). FONTCONFIG_FILE
# also reaches host programs, but nami's file includes the host config first.
if [ -d /etc/profile.d ]; then
  cat > /etc/profile.d/nami.sh <<EOF
# added by nami
export PATH="$U/bin:$P/bin:$V/bin:\$PATH"
export XDG_DATA_DIRS="$U/share:$P/share:\${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
# Cursor themes: libXcursor doesn't look in XDG_DATA_DIRS.
export XCURSOR_PATH="\$HOME/.local/share/icons:\$HOME/.icons:$U/share/icons:$P/share/icons:\${XCURSOR_PATH:-/usr/share/icons:/usr/share/pixmaps}"
export MANPATH="$U/share/man:$P/share/man:\${MANPATH:-}"
export LOCALE_ARCHIVE="$R/lib/locale/locale-archive"
export FONTCONFIG_FILE="$R/nami/fonts.conf"
# home.env / home.aliases from your nami file
if [ -r "\$HOME/.config/nami/env.sh" ]; then . "\$HOME/.config/nami/env.sh"; fi
EOF
fi
# Interactive non-login bash shells (most terminals) read bashrc, not profile.d.
for rc in /etc/bash.bashrc /etc/bashrc; do
  # (tray's line becomes nami's.)
  if [ -f "$rc" ] && grep -q 'tray/env.sh.*# tray$' "$rc"; then
    sed -i 's|/\.config/tray/env\.sh|/.config/nami/env.sh|g; s|  # tray$|  # nami|' "$rc"
  fi
  if [ -f "$rc" ] && ! grep -q nami/env.sh "$rc"; then
    printf '\nif [ -r "$HOME/.config/nami/env.sh" ]; then . "$HOME/.config/nami/env.sh"; fi  # nami\n' >> "$rc"
  fi
done
# Graphical sessions (GNOME, KDE, ...) take their environment from systemd,
# not from shell profiles; this is what makes menu entries and .desktop files work.
# With systemd: a generator, run as each session starts, says it. (Not an
# environment.d file: KDE Plasma's login on Fedora takes one's lines as they're
# written, ${HOME} and all, and Plasma doesn't start.)
if command -v systemctl >/dev/null && { [ -d /usr/lib/systemd/user-environment-generators ] || [ -d /lib/systemd/user-environment-generators ]; }; then
  rm -f /etc/environment.d/50-nami.conf
  mkdir -p /etc/systemd/user-environment-generators
  cat > /etc/systemd/user-environment-generators/50-nami <<EOF
#!/bin/sh
# added by nami: its apps, menus, cursors, fonts and locales in graphical sessions
U="\$HOME/.local/state/nami/profile"
echo "PATH=\$U/bin:$P/bin:$V/bin:\${PATH:-/usr/local/bin:/usr/bin:/bin}"
echo "XDG_DATA_DIRS=\$U/share:$P/share:\${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
echo "XCURSOR_PATH=\$HOME/.local/share/icons:\$HOME/.icons:\$U/share/icons:$P/share/icons:\${XCURSOR_PATH:-/usr/share/icons:/usr/share/pixmaps}"
echo "LOCALE_ARCHIVE=$R/lib/locale/locale-archive"
echo "FONTCONFIG_FILE=$R/nami/fonts.conf"
EOF
  chmod 755 /etc/systemd/user-environment-generators/50-nami
elif [ -d /usr/lib/environment.d ] || [ -d /etc/environment.d ] || command -v systemctl >/dev/null; then
  mkdir -p /etc/environment.d
  cat > /etc/environment.d/50-nami.conf <<EOF
PATH=\${HOME}/.local/state/nami/profile/bin:$P/bin:$V/bin:\${PATH}
XDG_DATA_DIRS=\${HOME}/.local/state/nami/profile/share:$P/share:\${XDG_DATA_DIRS:-/usr/local/share:/usr/share}
XCURSOR_PATH=\${HOME}/.local/share/icons:\${HOME}/.icons:\${HOME}/.local/state/nami/profile/share/icons:$P/share/icons:\${XCURSOR_PATH:-/usr/share/icons:/usr/share/pixmaps}
LOCALE_ARCHIVE=$R/lib/locale/locale-archive
FONTCONFIG_FILE=$R/nami/fonts.conf
EOF
fi
# fish doesn't read profile.d
if [ -d /etc/fish/conf.d ]; then
  cat > /etc/fish/conf.d/nami.fish <<EOF
fish_add_path -g \$HOME/.local/state/nami/profile/bin $P/bin $V/bin
set -gx XDG_DATA_DIRS \$HOME/.local/state/nami/profile/share $P/share \$XDG_DATA_DIRS
set -q XCURSOR_PATH; or set -gx XCURSOR_PATH /usr/share/icons /usr/share/pixmaps
set -gx XCURSOR_PATH \$HOME/.local/share/icons \$HOME/.icons \$HOME/.local/state/nami/profile/share/icons $P/share/icons \$XCURSOR_PATH
set -gx LOCALE_ARCHIVE $R/lib/locale/locale-archive
set -gx FONTCONFIG_FILE $R/nami/fonts.conf
EOF
fi

# Tab completion of nami's commands, packages, tasks... (`nami completions <shell>`), for
# each shell that's here, where it looks: bash-completion's folder, zsh's site-functions
# (compinit), fish's completions. (Each asks nami what fits: they stay as they are when
# nami updates itself.)
if command -v bash >/dev/null; then
  mkdir -p /usr/local/share/bash-completion/completions
  $V/bin/nami completions bash > /usr/local/share/bash-completion/completions/nami
fi
if command -v zsh >/dev/null; then
  mkdir -p /usr/local/share/zsh/site-functions
  $V/bin/nami completions zsh > /usr/local/share/zsh/site-functions/_nami
fi
if command -v fish >/dev/null; then
  mkdir -p /etc/fish/completions
  $V/bin/nami completions fish > /etc/fish/completions/nami.fish
fi

# nixpkgs looks for GPU drivers in /run/opengl-driver. /run is wiped at boot,
# so recreate the link every boot.
if [ -d /usr/lib/tmpfiles.d ] || [ -d /etc/tmpfiles.d ]; then
  mkdir -p /etc/tmpfiles.d
  echo "L+ /run/opengl-driver - - - - $R" > /etc/tmpfiles.d/nami.conf
elif [ -d /etc/local.d ]; then  # OpenRC (Alpine, Gentoo)
  printf '#!/bin/sh\n[ -e /run/opengl-driver ] || ln -sfn %s /run/opengl-driver\n' "$R" > /etc/local.d/nami.start
  chmod +x /etc/local.d/nami.start
elif [ -d /etc/runit ]; then  # Void
  printf '\n[ -e /run/opengl-driver ] || ln -sfn %s /run/opengl-driver\n' "$R" >> /etc/rc.local
fi
[ -e /run/opengl-driver ] || ln -sfn "$R" /run/opengl-driver

# What an admin's password is asked for, in the desktop's own box (nami's
# daemon asks polkit; without this, polkit's box says it's to run a program as
# root). Where /usr is read-only (Silverblue...), polkit's own words are used.
if [ -d /usr/share/polkit-1/actions ]; then
  cat 2>/dev/null > /usr/share/polkit-1/actions/org.namios.change-computer.policy <<'EOF' || true
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE policyconfig PUBLIC "-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN" "http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd">
<policyconfig>
  <vendor>nami</vendor>
  <vendor_url>https://namios.dev</vendor_url>
  <icon_name>nami</icon_name>
  <action id="org.namios.change-computer">
    <description>Change something for everyone on this computer</description>
    <message>nami wants to change something for everyone on this computer.</message>
    <defaults>
      <allow_any>auth_admin</allow_any>
      <allow_inactive>auth_admin</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
  </action>
</policyconfig>
EOF
  # tray's name for it, until 1.0: a tray daemon still running asks for this one.
  if [ -n "$had_tray" ]; then
    sed 's/org\.namios\.change-computer/org.trayos.change-computer/' /usr/share/polkit-1/actions/org.namios.change-computer.policy \
      > /usr/share/polkit-1/actions/org.trayos.change-computer.policy 2>/dev/null || true
  fi
fi

# The daemon lets users install into their own profiles without sudo.
# (tray's daemon goes: nami's takes its place.)
if command -v systemctl >/dev/null && [ -d /run/systemd/system ]; then
  if [ -e /etc/systemd/system/tray-daemon.service ]; then
    systemctl disable --now tray-daemon.service 2>/dev/null || true
    rm -f /etc/systemd/system/tray-daemon.service
  fi
  cat > /etc/systemd/system/nami-daemon.service <<EOF
[Unit]
Description=nami daemon (store access for users without root)
After=local-fs.target network-online.target
Wants=network-online.target
[Service]
ExecStart=$V/bin/nami daemon
Restart=on-failure
[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload && systemctl enable nami-daemon.service
  # Run by setup.sh (a release's install, `nami self-update`): it restarts the
  # daemon itself, last, once everything is in place. Run by itself: now.
  if [ -z "${NAMI_SETUP_RESTARTS:-}" ]; then
    systemctl restart nami-daemon.service
  fi
elif [ -d /etc/init.d ] && command -v openrc-run >/dev/null; then  # Alpine, Gentoo
  if [ -e /etc/init.d/tray-daemon ]; then
    rc-service tray-daemon stop 2>/dev/null || true; rc-update del tray-daemon default 2>/dev/null || true
    rm -f /etc/init.d/tray-daemon
  fi
  cat > /etc/init.d/nami-daemon <<EOF
#!/sbin/openrc-run
description="nami daemon"
command=$V/bin/nami
command_args=daemon
command_background=true
pidfile=/run/nami-daemon.pid
depend() { need localmount net; }
EOF
  chmod +x /etc/init.d/nami-daemon
  rc-update add nami-daemon default 2>/dev/null && rc-service nami-daemon start 2>/dev/null || true
elif [ -d /etc/sv ]; then  # Void (runit)
  rm -f /var/service/tray-daemon; rm -rf /etc/sv/tray-daemon
  mkdir -p /etc/sv/nami-daemon
  printf '#!/bin/sh
exec %s/bin/nami daemon 2>&1
' "$V" > /etc/sv/nami-daemon/run
  chmod +x /etc/sv/nami-daemon/run
  [ -d /var/service ] && ln -sf /etc/sv/nami-daemon /var/service/ || true
else
  echo "note: no init system detected; run 'nami daemon' as root for users without sudo"
fi

# Updates by themselves, once a day (nami-update.timer: nami, the catalog and the
# computer's apps), unless /etc/nami/system.nami says `updates: manual`. A computer
# with an older nami gets it here, as it updates to this one (setup.sh runs this).
if command -v systemctl >/dev/null && [ -d /etc/systemd ]; then
  $V/bin/nami os-update --set-up || echo "warning: nami won't update by itself; run: sudo nami os-update --set-up"
fi

echo "nami installed. Open a new shell, then: nami update --from <index>; nami install hello"
