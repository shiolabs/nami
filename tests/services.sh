#!/usr/bin/env sh
# `services:` and `files:`: in an Ubuntu container booted with systemd, a
# service of your own (a nixpkgs busybox httpd serving a file from /etc) is
# enabled and started; changing it restarts it, `nami undo` brings the old one
# back, a `restart-on:` file changing restarts it, removing it stops it and
# restores the file that was there. A user who
# isn't an admin runs a package's user unit (syncthing) and one of her own.
# Then, in a container without systemd: the services are left out (and it
# says why), the rest is set up, and `files:` still works.
# usage: tests/services.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
image=nami-test-systemd
docker build -q -t $image - >/dev/null <<'EOF'
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv dbus dbus-user-session ca-certificates xz-utils \
 && apt-get clean && rm -rf /var/lib/apt/lists/*
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
EOF
# podman runs systemd as is (--systemd=always); docker needs the cgroup tree.
if docker --version 2>/dev/null | grep -qi podman; then boot="--systemd=always"; else boot="--cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw --tmpfs /run --tmpfs /run/lock"; fi
ctr=$(docker run -d --privileged $boot -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro \
  -v "$index":/i.xz:ro -v "$PWD/tests/services-inner.sh":/run.sh:ro $image)
trap 'docker rm -f "$ctr" >/dev/null 2>&1' EXIT
docker exec "$ctr" systemctl is-system-running --wait >/dev/null 2>&1 || true   # "degraded" is fine
docker exec "$ctr" bash /run.sh systemd
echo "== without systemd"
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro \
  -v "$index":/i.xz:ro -v "$PWD/tests/services-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh none
