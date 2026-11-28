#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/bundle.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }
N=/nix/var/nix/profiles/default/bin
if [ "$MODE" != plain ]; then
  cp -a /seed/nix / && cp -a /seed/etc/nix /etc/
  printf 'build-users-group =\nsandbox = false\nssl-cert-file = %s\n' "$(ls /nix/store/*-nss-cacert-*/etc/ssl/certs/ca-bundle.crt | head -1)" >> /etc/nix/nix.conf
fi
if [ "$MODE" = nixos ]; then
  touch /etc/NIXOS && mkdir -p /etc/nixos && printf '{ ... }: {\n}\n' > /etc/nixos/configuration.nix
fi

check "sudo sh nami.run" "sh /nami.run > /tmp/install.log 2>&1"
sed 's/^/       /' /tmp/install.log | tail -6
T=/nix/var/nami/bin/nami
check "the catalog is there" "$T search firefox | grep -qi firefox"
check "it has icons" "test \$(ls /nix/var/nami/icons | wc -l) -gt 1000"
check "build definitions are there (unfree builds)" "test -e /nix/var/nami/catalog/drvs"
check "the unpacked installer was cleaned up" "! ls -d /var/tmp/nami-install.* 2>/dev/null"
case $MODE in
  nix) check "it shares Nix's store" "$T doctor | grep -q 'shared with Nix'"
       check "an app it installs is in Nix's database" "$T install hello >/dev/null 2>&1 && $N/nix-store --check-validity \$(readlink -f /nix/var/nami/profiles/default/bin/hello | cut -d/ -f1-4)" ;;
  nixos) check "nothing written to /etc/profile.d or systemd" "! test -e /etc/profile.d/nami.sh && ! test -e /etc/systemd/system/nami-daemon.service"
         check "it wrote /etc/nixos/nami.nix" "test -e /etc/nixos/nami.nix"
         check "and says how to import it" "grep -q 'imports = \[ ./nami.nix \]' /tmp/install.log"
         check "the module is valid Nix" "$N/nix-instantiate --parse /etc/nixos/nami.nix >/dev/null"
         check "and evaluates as a NixOS module" "$N/nix-instantiate --eval --strict -E '(import /etc/nixos/nami.nix { config.nix.package = \"nix\"; }).systemd.services.nami-daemon.serviceConfig.ExecStart' | grep -q nami" ;;
esac
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
