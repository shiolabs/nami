#!/usr/bin/env sh
# Bundles across the rename (docs/rename-migration.md): bundles made by tray
# before it was nami carry a runtime that knows only tray's names
# (--tray-bundle-*, TRAY_BUNDLE_*, $XDG_CACHE_HOME/tray-bundle, tray-bundle-<id>.desktop,
# .<file>.tray-update); nami 0.18.6 to 0.18.8's know only nami's. Old and new
# bundles, and old and new nami, must keep working together.
#
#   make     in an Ubuntu container (one store, as on a developer's computer):
#            tray (the old one: TRAY_OLD) bundles ripgrep 15.1.0 with
#            --update-url, htop and jq; then nami (this one) runs, which
#            moves tray's places, and publishes into the same feeds:
#            ripgrep 15.1.0 again (a new runtime, the same parts), then
#            15.2.0 (patches against the release before, which tray
#            published); a feed of nami's that tray then publishes into
#            (the reverse); htop, jq as an AppImage, busybox (a server)
#   run      on Ubuntu 24.04 and Fedora, as someone else, no nami: one store
#            for both kinds (and with XDG_CACHE_HOME, and nami 0.18.6-0.18.8's
#            folder there); each cleaning up never takes the other's parts;
#            tray's words to a new bundle; updates old to new (whole parts),
#            then new to newer (patches, on parts the old runtime unpacked),
#            an old one's own background update into a new one, and new to
#            old; updates turned off by the old one stay off; the menu entry
#            an old one made is the new one's; the install line over an old copy
#   host     nami (this one) as the host: `nami install` of an old bundle
#            (unpacked by its own words, not started), its menu entry in
#            place of the old runtime's, `nami file-apps --update`, `nami remove`
#            (its parts cleaned)
#   oldhost  tray (the old one) as the host of a new bundle: installed and removed
#
# usage: TRAY_OLD=<static tray from before the rename, with its runtimes attached>
#        [NAMI=target/release/nami] [INDEX=/nix/var/nami/index.json]
#        [OUT=target/bundle-rename] [NIX_VOLUME=nami-rename-nix]
#        tests/bundle-rename.sh [make|run|host|oldhost ...]
#   (TRAY_OLD: e.g. `git archive 091efc49 | tar x -C old && (cd old && ./build.sh)`,
#   the last commit before the rename: old/target/release/tray.)
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"
nami=$(realpath "${NAMI:-target/release/nami}")
tray=$(realpath "${TRAY_OLD:?TRAY_OLD: a static tray from before the rename}")
index=$(realpath "${INDEX:-/nix/var/nami/index.json}")
mkdir -p "${OUT:-target/bundle-rename}"
out=$(realpath "${OUT:-target/bundle-rename}")
vol=${NIX_VOLUME:-nami-rename-nix}
images=${RUN_IMAGES:-"ubuntu:24.04 fedora:latest"}
stages=${*:-make run host oldhost}
fail=0

make_bundles() {
  echo "== make (in ubuntu:24.04): tray's bundles, then nami's"
  docker volume create $vol >/dev/null
  docker run --rm --privileged -v $vol:/nix -v "$nami":/usr/local/bin/nami:ro -v "$tray":/usr/local/bin/tray:ro \
    -v "$index":/index.json:ro -v "$out":/out ubuntu:24.04 bash -c '
    set -eu
    RG1=jmxrpp9mj5l669s0y4f4rfdh10ln51xj-ripgrep-15.1.0 RG2=vli6118fdbngwy46j1w741ansmrzfszr-ripgrep-15.2.0
    url=http://127.0.0.1:8473
    cd /out && rm -rf t c chain chain@* direct back busybox-x86_64 && mkdir -p t c
    b() { # b <who> <log> <bundle arguments...>
      who=$1 log=$2; shift 2; s=$(date +%s)
      $who bundle "$@" >/out/$log.log 2>&1 || { tail -20 /out/$log.log; exit 1; }
      echo "   $who bundle $* ($(( $(date +%s) - s ))s)"
      grep -E "^(since|no patch)" /out/$log.log | sed "s/^/     /" || true
    }
    # tray, as a computer had it before nami (the catalog in its own places).
    tray update --from /index.json --unsigned >/dev/null
    b tray t-chain /nix/store/$RG1 --update-url $url/chain --publish-dir /out/chain -o /out/t/rg-chain
    cp -r /out/chain /out/chain@1
    b tray t-direct /nix/store/$RG1 --update-url $url/direct --publish-dir /out/direct -o /out/t/rg-direct
    b tray t-htop htop -o /out/t/htop
    echo "   tray signs with: $(tray bundle --show-signing-key 2>/dev/null)"
    # nami: it moves tray'"'"'s places (/nix/var/tray, ~/.config/tray), and publishes into tray'"'"'s feeds.
    nami update --from /index.json --unsigned >/dev/null
    echo "   after nami: ~/.config/tray -> $(readlink /root/.config/tray), /nix/var/tray -> $(readlink /nix/var/tray)"
    echo "   nami signs with: $(nami bundle --show-signing-key 2>/dev/null)"
    b nami c-chain-2 /nix/store/$RG1 --update-url $url/chain --publish-dir /out/chain -o /out/c/rg-chain-2
    cp -r /out/chain /out/chain@2
    b nami c-chain-3 /nix/store/$RG2 --update-url $url/chain --publish-dir /out/chain -o /out/c/rg-chain-3
    grep -q "^since 15.1.0: .* as patches" /out/c-chain-3.log || { echo "   15.2.0: no patches against the release before"; exit 1; }
    b nami c-direct /nix/store/$RG2 --update-url $url/direct --publish-dir /out/direct -o /out/c/rg-direct-2
    grep -q "^since 15.1.0: .* as patches" /out/c-direct.log || { echo "   15.2.0: no patches against tray'"'"'s release"; exit 1; }
    b nami c-back /nix/store/$RG1 --update-url $url/back --publish-dir /out/back -o /out/c/rg-back
    b nami c-htop htop -o /out/c/htop
    b nami c-jq jq --appimage -o /out/c/jq
    b nami busybox busybox -o /out/busybox-x86_64
    # tray again, after nami: the same key, into nami'"'"'s feed (an update back to a bundle tray made).
    b tray t-back /nix/store/$RG2 --update-url $url/back --publish-dir /out/back -o /out/t/rg-back-2
    [ "$(tray bundle --show-signing-key 2>/dev/null)" = "$(nami bundle --show-signing-key 2>/dev/null)" ] || { echo "   tray and nami sign with different keys"; exit 1; }
    ls -l /out/t /out/c | sed "s/^/   /"' || fail=$((fail + 1))
}

run_checks() {
  for image in $images; do
    echo "== run on $image, as someone else"
    docker run --rm -u 1234:1234 -e HOME=/tmp/h -v "$out":/b:ro -v "$PWD/tests/bundle-rename-inner.sh":/run.sh:ro "$image" sh /run.sh run || fail=$((fail + 1))
  done
}

host() {
  echo "== nami as the host of tray's bundles (ubuntu:24.04, as someone else)"
  docker run --rm -u 1234:1234 -e HOME=/tmp/h -v "$out":/b:ro -v "$nami":/usr/local/bin/nami:ro -v "$PWD/tests/bundle-rename-inner.sh":/run.sh:ro ubuntu:24.04 sh /run.sh host || fail=$((fail + 1))
}

oldhost() {
  echo "== tray (before the rename) as the host of nami's bundles (ubuntu:24.04, as someone else)"
  docker run --rm -u 1234:1234 -e HOME=/tmp/h -v "$out":/b:ro -v "$tray":/usr/local/bin/tray:ro -v "$PWD/tests/bundle-rename-inner.sh":/run.sh:ro ubuntu:24.04 sh /run.sh oldhost || fail=$((fail + 1))
}

for s in $stages; do
  case $s in
    make) make_bundles ;;
    run) run_checks ;;
    host) host ;;
    oldhost) oldhost ;;
    *) echo "unknown stage $s"; exit 2 ;;
  esac
done
[ $fail = 0 ] && echo "== all passed" || echo "== $fail failed"
[ $fail = 0 ]
