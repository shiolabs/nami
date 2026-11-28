#!/bin/sh
# Runs a command with the catalog's tools from nami (index/project.nami), on
# a computer without Nix or Docker (namiOS). build-index.sh and
# packaging/catalog-update.sh use it:
#   nami run --project index -- sh index/nami-env.sh <command...>
#
# Nix keeps a store of its own in ~/.cache/nami-index/nix (NAMI_INDEX_STORE):
# the system's /nix/store and /nix/var are never touched. The paths inside
# it are still /nix/store/..., so every drvPath and output path is the one
# cache.nixos.org has, as in the container.
#
# nami puts python314Packages' packages in the store but not where Python
# looks, and the scripts run python3 -I (no PYTHONPATH): a small venv here
# lists them in a .pth file, which -I still reads.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
store=${NAMI_INDEX_STORE:-$HOME/.cache/nami-index/nix}
mkdir -p "$store/etc"
printf '%s\n' 'experimental-features = nix-command flakes' 'substituters = https://cache.nixos.org' \
  'trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFNspFhjjbY=' 'sandbox = false' \
  > "$store/etc/nix.conf"
# NIX_REMOTE, not NIX_CONFIG's store: history.py and in-container.sh set NIX_CONFIG.
export NIX_REMOTE="local?root=$store" NIX_STORE_DIR=/nix/store NIX_STATE_DIR="$store/nix/var/nix" \
  NIX_LOG_DIR="$store/nix/var/log/nix" NIX_CONF_DIR="$store/etc" NIX_CACHE_HOME="$store/cache"

py=$store/python
[ -x "$py/bin/python3" ] && [ "$(readlink -f "$py/bin/python3")" = "$(readlink -f "$(command -v python3)")" ] ||
  { rm -rf "$py"; python3 -m venv --without-pip "$py"; }
site=$("$py/bin/python3" -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')
jq -r '.packages | to_entries[] | select(.key | startswith("python3")) | .value.paths[]' "$here/nami.lock" |
  while read -r p; do
    for d in "$p"/lib/python3*/site-packages; do [ ! -d "$d" ] || echo "$d"; done
  done > "$site/nami.pth"
PATH=$py/bin:$PATH
exec "$@"
