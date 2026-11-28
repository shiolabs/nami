#!/bin/sh
# Runs chrooted in an arm64 distro's files, aarch64 programs emulated (see
# tests/bundle-arm.sh); the bundles are in /b.
. /etc/os-release
export HOME=/tmp/home PATH=/usr/sbin:/usr/bin:/sbin:/bin
mkdir -p $HOME
fails=0
echo "== run on $PRETTY_NAME ($(uname -m), glibc $(ldd --version 2>/dev/null | head -1 | sed 's/.* //'))"
check() { if out=$(eval "$2" 2>&1); then echo "  ok   $1"; else echo "  FAIL $1"; echo "$out" | tail -5 | sed 's/^/       /'; fails=$((fails + 1)); fi; }
check "no /nix here" "test ! -e /nix"
check "jq runs" "echo '{\"a\":[1,2,3]}' | /b/jq-aarch64 -c '.a | add' | grep -qx 6"
check "on the bundle's own aarch64 glibc, not the distro's" "LD_DEBUG=libs /b/jq-aarch64 --version 2>&1 | grep 'calling init' | grep -q '/tmp/.....//*[a-z0-9]*-glibc-2.*/lib/libc.so.6'"
check "it says what it is" "/b/jq-aarch64 --nami-bundle-info | head -1 | grep -q '^jq '"
check "it checks out against its hash" "/b/jq-aarch64 --nami-bundle-verify"
check "tmux runs" "/b/tmux-aarch64 -V | grep -q '^tmux '"
check "a thin jq fetches its aarch64 parts from cache.nixos.org, checked" "rm -rf \$HOME/.cache; echo '[3,4]' | /b/jq-thin-aarch64 -c add | grep -qx 7"
[ "$fails" -eq 0 ] && echo "  PASS" || { echo "  $fails FAILED"; exit 1; }
