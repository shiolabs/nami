#!/bin/sh
# Check extra-app recipes (packaging/apps/<name>) the way a release builds
# them, without keys or publishing: `nami build` in a scratch root, against
# the live signed catalog, then what Nami would show for each (name,
# summary, publisher, license, icon, what it downloads), as Markdown. CI runs
# this on pull requests that touch packaging/apps (.github/workflows/apps.yml).
#
# Exit status: 0 all good, 1 a recipe doesn't build or isn't an app,
# 2 builds but is missing something people would see (no icon, summary...).
# Needs user namespaces (unshare -r), like packaging/publish-apps.sh.
# usage: [CHANNEL=...] [KEY=name:base64] [NAMI=target/release/nami] [CHECK_ROOT=dir]
#        packaging/check-app.sh <recipe-dir>...   (default: every recipe)
set -eu
# A test: namios.dev counts its update checks and downloads apart (NAMI_TEST, site/worker.js).
export NAMI_TEST=1
cd "$(dirname "$0")/.."
CHANNEL=${CHANNEL:-https://namios.dev/menu/nixos-unstable}
KEY=${KEY:-tray-2:uHz+KI06voKMb/xTAfd9nd4d8y008M5x5xWFy/pbpIs=}
NAMI=$(realpath "${NAMI:-target/release/nami}")
[ $# -gt 0 ] || set -- packaging/apps/*/
root=${CHECK_ROOT:-$(mktemp -d)}
mkdir -p "$root"
t() { NAMI_ROOT=$root unshare -r "$NAMI" "$@"; }
t update --from "$CHANNEL" --trust "$KEY" >/dev/null

log=$(mktemp); trap 'rm -f "$log"' EXIT
status=0
for recipe; do
  recipe=${recipe%/}
  app=$(basename "$recipe")
  if [ ! -e "$recipe/package.nami" ]; then
    printf '### %s\n\n**Not added:** no package.nami\n\n' "$app"; status=1; continue
  fi
  if ! out=$(t build "$recipe" 2>"$log"); then
    printf '### %s\n\n**Build failed:**\n\n```\n%s\n```\n\n' "$app" "$(tail -n 30 "$log")"; status=1; continue
  fi
  out=$(printf '%s\n' "$out" | tail -n 1)
  python3 -I packaging/extra-apps.py --check "$root/nix/var/nami/index.json" "$recipe" "$out" "$root$out" || case $? in
    2) [ $status -eq 1 ] || status=2 ;;
    *) status=1 ;;
  esac
done
exit $status
