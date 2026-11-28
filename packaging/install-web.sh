#!/bin/sh
# nami installer: curl -fsSL @BASE@/install | sh
# Downloads the latest release, checks it, and installs it (asks for sudo).
# The package catalog comes from @BASE@/menu/@CHANNEL@, signed by one of the
# keys below; nami refuses a catalog that isn't.
#
# Options: curl -fsSL @BASE@/install | sh -s -- --config <repo>
#   --config <repo>  then set this computer up from your config repo
#                (github:you/nami-config, or any git URL): `nami apply <repo>`
#   --no-app     the command line only, without nami's app (nami-gui), which
#                otherwise comes where there's a desktop. Or NAMI_NO_APP=1:
#                curl -fsSL @BASE@/install | NAMI_NO_APP=1 sh
#                Add the app later with: sudo nami install nami-gui
#                Remove it with: sudo nami remove nami-gui
#                (On a Mac the app is nami.app, in Applications: move it to the
#                Trash to remove it.)
set -eu
BASE="@BASE@"
CHANNEL="@CHANNEL@"
KEY="@KEY@"     # what releases are signed with
# Every key nami's site is signed with (packaging/keys). The computer trusts
# those in use: the release's and the catalog's (they differ while a new key
# comes in).
KEYS="@KEYS@"

config=""
case ${NAMI_NO_APP:-} in ""|0|no|false) app=1 ;; *) app="" ;; esac
# NAMI_TEST=1: a test computer (a VM, a container). The site counts its
# downloads and update checks as tests' (?via=test), and nami's too: the
# installer leaves /nami/test-computer (namios.dev/privacy), which says so
# from then on, installing again too.
[ ! -e /nix/var/nami/test-computer ] || NAMI_TEST=1
case ${NAMI_TEST:-} in ""|0|no|false) test="" via=install ;; *) test="?via=test" via=test ;; esac
while [ $# -gt 0 ]; do
  case $1 in
    -y|--yes) ;;   # nothing to ask any more
    --no-app) app="" ;;
    --config=*) config=${1#--config=} ;;
    --config) [ $# -gt 1 ] || { echo "--config needs your config repo (github:you/nami-config)"; exit 1; }; config=$2; shift ;;
    -h|--help) printf '%s\n' "nami installer." "  --config <repo>  then set this computer up from your config repo (github:you/nami-config)" "  --no-app         the command line only, without nami's app (or NAMI_NO_APP=1)"; exit 0 ;;
    *) echo "unknown option: $1 (try --help)"; exit 1 ;;
  esac
  shift
done

# Colours and redrawing only on a terminal.
if [ -t 1 ] && [ "${NO_COLOR:-}" = "" ]; then
  B=$(printf '\033[1m') D=$(printf '\033[2m') G=$(printf '\033[32m') R=$(printf '\033[31m') Y=$(printf '\033[33m') N=$(printf '\033[0m')
else
  B="" D="" G="" R="" Y="" N=""
fi
say() { printf '  %s\n' "$*"; }
ok() { printf '  %s✓%s %s\n' "$G" "$N" "$*"; }
fail() { printf '  %s✗ %s%s\n' "$R" "$*" "$N"; exit 1; }

# The computer: x86_64 or ARM (aarch64) Linux, or a Mac (a preview: packages and projects).
machine=$(uname -m)
# (An Apple silicon Mac's Terminal under Rosetta says x86_64: it's arm64 all the same.)
[ "$(uname -s)" = Darwin ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ] && machine=arm64
case "$(uname -s)-$machine" in
  Linux-x86_64) system=x86_64-linux tagline="your whole Linux setup in one file" ;;
  Linux-aarch64|Linux-arm64) system=aarch64-linux tagline="your whole Linux setup in one file" ;;
  Darwin-arm64) system=aarch64-darwin tagline="your packages and projects in one file (Mac preview)" ;;
  Darwin-x86_64) system=x86_64-darwin tagline="your packages and projects in one file (Mac preview)" ;;
  *) system="" tagline="" ;;
esac
printf '\n  %snami%s  %s%s%s\n\n' "$B" "$N" "$D" "$tagline" "$N"
[ -n "$system" ] || fail "nami runs on x86_64 and ARM (aarch64) Linux and on Macs for now. This computer is $(uname -s) $(uname -m)."
# A Mac: macOS 14 (Sonoma) or newer, what nixpkgs' programs are made for (since 25.11):
# on an older one they don't start.
if [ "${system#*-}" = darwin ]; then
  macos=$(sw_vers -productVersion 2>/dev/null || echo 0)
  [ "${macos%%.*}" -ge 14 ] 2>/dev/null || fail "nami needs macOS 14 (Sonoma) or newer: the programs it installs are made for it. This Mac has macOS $macos."
fi
# nami's app: nami-gui on Linux (x86_64 and ARM), nami.app on a Mac.
# ARM: memory pages other than 4 KB (Asahi Linux's 16 KB) break some of nixpkgs' programs.
# nami's app hasn't been tried on such a computer: it's left out there unless you add it.
if [ "$system" = aarch64-linux ] && [ "$(getconf PAGESIZE 2>/dev/null || echo 4096)" != 4096 ]; then
  say "${Y}!${N} This computer uses $(( $(getconf PAGESIZE) / 1024 )) KB memory pages (Asahi Linux does). Some programs from nixpkgs are made for 4 KB pages and won't start on it."
  say "  nami's app hasn't been tested with them, so the installer leaves it out."
  app=""
fi
# sha256sum, or a Mac's shasum.
if command -v sha256sum >/dev/null 2>&1; then sha256() { sha256sum "$@"; }; else sha256() { shasum -a 256 "$@"; }; fi
if command -v curl >/dev/null 2>&1; then fetch() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then fetch() { wget -qO- "$1"; }
else fail "The installer needs curl or wget."
fi
if [ "$(id -u)" -eq 0 ]; then SUDO=""
elif command -v sudo >/dev/null 2>&1; then SUDO=sudo
else fail "Run this as root (there's no sudo)."
fi
# A Mac account that isn't an administrator can't use sudo: macOS's own box asks for an
# administrator's name and password instead, once, for all of the installing (as nami's
# app does to uninstall nami). Their account uses nami as any other does: installing
# without sudo goes through nami's daemon.
mac_admin=""
if [ -n "$SUDO" ] && [ "${system#*-}" = darwin ] && ! id -Gn | tr ' ' '\n' | grep -qx admin; then
  mac_admin=1
  say "${Y}!${N} This account ($(id -un)) isn't an administrator, so macOS will ask for an"
  say "  administrator's name and password to install nami."
  echo
fi

version=$(fetch "$BASE/releases/latest$test") || fail "Couldn't reach $BASE. Check your connection and try again."
file="nami-$version-$system.tar.gz"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
# ?via=install: the site counts it as a new install, not an update (namios.dev/privacy).
fetch "$BASE/releases/$version/$file?via=$via" > "$tmp/$file" || fail "Downloading nami $version failed. Check your connection and try again."
size=$(du -h "$tmp/$file" | cut -f1 | tr -d " ")   # (a Mac's du pads it: " 15M")
ok "Downloaded nami $version ($size)"
fetch "$BASE/releases/$version/SHA256SUMS" > "$tmp/SHA256SUMS" || fail "Downloading the checksums failed."
fetch "$BASE/releases/$version/SHA256SUMS.sig" > "$tmp/SHA256SUMS.sig" || fail "Downloading the checksums' signature failed."
(cd "$tmp" && [ "$(grep " $file\$" SHA256SUMS | cut -d' ' -f1)" = "$(sha256 "$file" | cut -d' ' -f1)" ]) || fail "The download doesn't match its checksum; nothing was installed."
# key_of <name>: the key of that name, from KEYS.
key_of() { for k in $KEYS; do [ "${k%%:*}" = "$1" ] && { echo "$k"; return 0; }; done; return 1; }
signer() { cut -d: -f1 "$1" | head -1 | tr -d ' \r\n'; }
# The checksums are signed with a key above (as for nami self-update):
# checked with openssl (3.0 or newer), where there is one.
rel_signer=$(signer "$tmp/SHA256SUMS.sig")
rel_key=$(key_of "$rel_signer") || rel_key=""
verify() {
  [ -n "$rel_key" ] || return 1
  { printf '\060\052\060\005\006\003\053\145\160\003\041\000'; printf '%s' "${rel_key#*:}" | base64 -d; } > "$tmp/key.der"
  cut -d: -f2- "$tmp/SHA256SUMS.sig" | tr -d '\n' | base64 -d > "$tmp/sig"
  # (The context stays tray's, as nami signs and checks it: src/catalog.rs.)
  printf 'tray-catalog-v1:%s' "$(sha256 "$tmp/SHA256SUMS" | cut -d' ' -f1)" > "$tmp/msg"
  openssl pkeyutl -verify -pubin -keyform DER -inkey "$tmp/key.der" -rawin -in "$tmp/msg" -sigfile "$tmp/sig" >/dev/null 2>&1
}
if command -v openssl >/dev/null 2>&1 && openssl pkeyutl -help 2>&1 | grep -q rawin; then
  [ -n "$rel_key" ] || fail "The download is signed by ${rel_signer:-nobody}, a key this installer doesn't know; nothing was installed."
  verify || fail "The download isn't signed by $rel_signer; nothing was installed."
  ok "Verified ${D}(signed by namios.dev)${N}"
else
  ok "Checksum matches"
fi
# The key the catalog is signed with, for setup.sh to trust (nami checks
# the catalog's signature itself).
cat_key=$KEY
# (A Mac's catalog has a channel file of its own, menu/<channel>.<system>, when it's
# made apart from Linux's.)
rev=$(fetch "$BASE/menu/$CHANNEL.$system$test" 2>/dev/null || fetch "$BASE/menu/$CHANNEL$test" 2>/dev/null) && fetch "$BASE/menu/$rev/index-$system.json.xz.sig" > "$tmp/catalog.sig" 2>/dev/null &&
  cat_key=$(key_of "$(signer "$tmp/catalog.sig")") || cat_key=$KEY
tar -xzf "$tmp/$file" -C "$tmp"

test_note="A test computer (installed with NAMI_TEST=1): its update checks and downloads are counted as tests'."
if [ -n "$mac_admin" ]; then
  # All that needs root, in one script, for macOS's box to run as root; its output goes
  # to a file, shown as it comes.
  q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
  {
    echo 'set -u'
    # (Yours again after, whatever happens: the installer cleans up as you.)
    echo "trap $(q "chown -R $(id -u) $(q "$tmp")") EXIT"
    echo "env NAMI_CHANNEL=$(q "$BASE/menu/$CHANNEL") NAMI_KEY=$(q "$cat_key") NAMI_TEST=$(q "${test:+1}") NAMI_APP=$(q "${app:+1}") sh $(q "$tmp/nami-$version/setup.sh") || exit 1"
    echo 'V=$(cd /nix/var/nami && pwd -P) || exit 1'
    [ -z "$test" ] || echo "echo $(q "$test_note") > \"\$V/test-computer\""
    [ -z "$rel_key" ] || [ "$rel_key" = "$cat_key" ] ||
      echo "\"\$V/bin/nami\" update --from $(q "$BASE/menu/$CHANNEL") --trust $(q "$rel_key") 2>&1 | grep '^trusting'"
    echo 'exit 0'
  } > "$tmp/as-admin.sh"
  : > "$tmp/log"
  say "${Y}→${N} Installing nami in /nix ${D}(macOS asks for an administrator's name and password)${N}"
  echo
  /usr/bin/osascript -e 'on run argv' \
    -e 'do shell script ("/bin/sh " & quoted form of item 1 of argv & " >> " & quoted form of item 2 of argv & " 2>&1") with prompt "nami wants to install itself on this Mac." with administrator privileges' \
    -e 'end run' "$tmp/as-admin.sh" "$tmp/log" </dev/null > "$tmp/osascript" 2>&1 &
  asking=$!
  # The log's new lines, indented under ours, until the script is done.
  shown=0
  show() { n=$(wc -l < "$tmp/log" | tr -d ' '); [ "$n" -le "$shown" ] || { sed -n "$((shown + 1)),${n}p" "$tmp/log" | sed "s/^/    ${D}│${N} /"; shown=$n; }; }
  while kill -0 "$asking" 2>/dev/null; do show; sleep 0.3; done
  wait "$asking" && st=0 || st=$?
  show; sed -n "$((shown + 1)),\$p" "$tmp/log" | sed "s/^/    ${D}│${N} /"
  if [ "$st" != 0 ]; then
    grep -q '(-128)' "$tmp/osascript" && fail "Not installed: macOS's box was cancelled. Run the installer again to retry."
    [ -s "$tmp/log" ] && fail "Installing didn't finish (see above). Run the installer again to retry."
    say "${D}$(cat "$tmp/osascript")${N}"
    fail "Not installed: macOS couldn't ask for an administrator's password here. Log in to an administrator's account (or, in Terminal: su <their name>) and run the installer again."
  fi
  V=$(cd /nix/var/nami && pwd -P)
else
if [ -n "$SUDO" ]; then
  say "${Y}→${N} Installing needs your password (sudo):"
  $SUDO -v || fail "Not installed: sudo didn't work."
fi
say "${Y}→${N} Installing nami in /nix"
echo
# setup.sh's own progress, indented under ours.
# (NAMI_APP: a Mac's app, nami.app, comes in the release, and install-mac.sh puts it in
# /Applications unless you said --no-app.)
( $SUDO env NAMI_CHANNEL="$BASE/menu/$CHANNEL" NAMI_KEY="$cat_key" NAMI_TEST="${test:+1}" NAMI_APP="${app:+1}" \
    sh "$tmp/nami-$version/setup.sh" 2>&1 && echo 0 > "$tmp/status" || echo 1 > "$tmp/status" ) | sed "s/^/    ${D}│${N} /"
[ "$(cat "$tmp/status")" = 0 ] || fail "Installing didn't finish (see above). Run the installer again to retry."
# nami's folder: /nami, or /nix/var/nami where / can't hold it (/nix/var/nami reaches it).
V=$(cd /nix/var/nami && pwd -P)
[ -z "$test" ] || echo "$test_note" | $SUDO tee "$V/test-computer" >/dev/null
# The release's key too, where the catalog's is another: nami's next
# releases are signed with it (nami self-update), and so is its next catalog.
if [ -n "$rel_key" ] && [ "$rel_key" != "$cat_key" ]; then
  ( $SUDO $V/bin/nami update --from "$BASE/menu/$CHANNEL" --trust "$rel_key" 2>&1 | grep '^trusting' ) | sed "s/^/    ${D}│${N} /"
fi
fi
if [ -n "$rel_key" ] && [ "$rel_key" != "$cat_key" ]; then
  grep -q "\"$rel_key\"" $V/catalog-keys.json 2>/dev/null ||
    say "${R}✗${N} nami doesn't trust ${rel_key%%:*}, which its updates are signed with. Add it with: ${B}sudo nami update --from $BASE/menu/$CHANNEL --trust $rel_key${N}"
fi
echo
ok "${B}nami $version is installed.${N}"
# nami's app (nami's window), where there's a desktop to show it on, unless
# you said --no-app: `nami install nami-gui` puts its recipe (from nami's
# examples) in /etc/nami/packages and lists it in /etc/nami/system.nami. Once
# taken out (`sudo nami remove nami-gui`, which leaves the recipe), it stays
# out, here and in updates.
# A desktop: one in use, or one to log in to (Wayland's or X11's: either is enough).
has_desktop() {
  [ -n "${XDG_CURRENT_DESKTOP:-}${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] && return 0
  for f in /usr/share/wayland-sessions/*.desktop /usr/share/xsessions/*.desktop; do [ -e "$f" ] && return 0; done
  return 1
}
gui=""
if [ "${system#*-}" = darwin ]; then
  # A Mac's: nami.app, in Applications now unless you said --no-app (install-mac.sh), when
  # this release has it.
  if [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' /Applications/nami.app/Contents/Info.plist 2>/dev/null)" = dev.namios.nami ]; then
    gui=mac-installed
    ok "nami's app is in your Applications folder (${B}nami${N})"
  elif [ -d "$tmp/nami-$version/nami.app" ]; then
    gui=mac-skipped
  fi
elif [ "$system" != x86_64-linux ] && ! grep -q " nami-gui-$version-$system.tar.gz\$" "$tmp/SHA256SUMS"; then
  # (An ARM release made without ARM's app has nothing to offer. x86_64's always has it:
  # packaging/publish.sh makes none without it.)
  :
elif grep -qE '^[[:space:]]*-[[:space:]]*["'"'"']?(\./packages/|/etc/nami/packages/)?nami-gui["'"'"']?[[:space:]]*(#.*)?$' /etc/nami/system.nami 2>/dev/null; then
  gui=installed   # (setup.sh brought it up to date)
elif [ -z "$app" ] || [ -d /etc/nami/packages/nami-gui ]; then
  gui=skipped
elif has_desktop; then
  say "${Y}→${N} Installing nami's app"
  ( $SUDO "$V/bin/nami" install nami-gui 2>&1 && echo 0 > "$tmp/status" || echo 1 > "$tmp/status" ) | sed "s/^/    ${D}│${N} /"
  if [ "$(cat "$tmp/status")" = 0 ]; then
    gui=installed
    ok "nami's app is in your app menu (${B}nami-gui${N})"
  else
    gui=skipped
    say "${R}✗${N} The app didn't install (see above). nami works without it."
  fi
fi
if [ -n "$config" ]; then
  # As you, not root: the repo becomes your ~/.config/nami. Its
  # The computer's file (its folder, named after it) goes to the computer through nami's daemon (admins).
  say "${Y}→${N} Setting this computer up from ${B}$config${N}"
  echo
  if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ]; then
    as_you="sudo -u $SUDO_USER -H"
  else
    as_you=""
  fi
  # stdin is this script (curl | sh): git and ssh ask on the terminal itself.
  ( $as_you $V/bin/nami apply "$config" </dev/null 2>&1 && echo 0 > "$tmp/status" || echo 1 > "$tmp/status" ) | sed "s/^/    ${D}│${N} /"
  echo
  if [ "$(cat "$tmp/status")" = 0 ]; then
    ok "Set up from $config"
  else
    say "${R}✗${N} Setting up from $config didn't finish (see above). nami is installed; retry with: ${B}nami apply $config${N}"
  fi
fi
case $system in *-linux)
  say "  Open a new terminal, then try: ${B}nami install firefox${N}"
  say "  Log out and back in once, so your app menu shows nami's apps."
  ;; *)
  # (A Mac: programs for the terminal for now; nami doesn't put the apps it installs in
  # /Applications yet, only its own.)
  say "  Open a new terminal, then try: ${B}nami install ripgrep${N}"
esac
say "  New to nami? ${B}$BASE/docs/nami/first-apps${N}"
case $gui in
  installed) say "  ${D}Don't want the app? Remove it with: sudo nami remove nami-gui${N}" ;;
  skipped) say "  ${D}Want the app? Add it with: sudo nami install nami-gui${N}" ;;
  mac-installed) say "  ${D}Don't want the app? Move nami from Applications to the Trash.${N}" ;;
  mac-skipped) say "  ${D}Want the app? Run the installer again without --no-app.${N}" ;;
esac
if [ -n "$mac_admin" ]; then
  say "  ${D}Uninstall nami again from nami's app: nami › Uninstall nami… (it asks for an administrator)${N}"
else
  say "  ${D}Uninstall nami again with: sudo nami self-uninstall${N}"
fi
echo
