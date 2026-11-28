#!/bin/sh
# nami against Nix: the numbers on the front page (site/src/pages/index.astro,
# "nami and Nix"), measured again on your computer.
#
# Each measurement runs in a new container, so nothing is cached from before:
# nami in ubuntu:24.04 (the static nami binary, nothing else), Nix in the
# official nixos/nix image. Both get the same nixpkgs: Nix is pinned to the
# nixpkgs revision nami's catalog was made from, and both download the same
# ready-built packages from cache.nixos.org.
#
#   Search all packages             nami search ripgrep    nix search <nixpkgs> ripgrep
#                                   (after the catalog,    (after a first search, which
#                                   or nixpkgs, is there)  fetches nixpkgs)
#   New computer, three tools       nami update (the       nix profile add ripgrep jq htop,
#     installed                     catalog) + nami        the first command in the container
#                                   install ripgrep jq htop
#   Start a project's tools again   nami run -- bat        nix shell <nixpkgs>#bat -c bat
#                                   --version in a folder  --version
#                                   with project.nami
#                                   (third run)            (third run)
#   What you install                the nami binary        the closure of the image's nix
#
# Every measurement is repeated ROUNDS times (each in new containers), and the
# middle value is reported, as the site does. Times are wall-clock
# milliseconds taken inside the container, so the container's start isn't
# counted. They depend on your network and computer; the ratios are what to
# compare.
#
# usage: tests/bench/nami-vs-nix.sh [nami binary]
#   With no binary, the newest release is downloaded from $BASE and checked
#   against its SHA256SUMS. A self-built one: ./build.sh, then
#   tests/bench/nami-vs-nix.sh target/release/nami
# Environment: ROUNDS (default 3), BASE (default https://namios.dev),
#   CHANNEL (default nixos-unstable), NIX_IMAGE (default
#   docker.io/nixos/nix:latest), UBUNTU_IMAGE (default docker.io/library/ubuntu:24.04),
#   KEEP=1 keeps the work folder (each container's output is in it).
# Needs: docker, or podman (tests/lib/docker uses it when docker isn't
# there), curl, sha256sum, tar. About 3 GB of downloads per round, most of
# it Nix's (nixpkgs and its packages); 5 to 10 minutes for three rounds.
set -eu
here=$(cd "$(dirname "$0")" && pwd -P)
# The docker stand-in (tests/lib/docker), by its real path: it skips only itself.
PATH="$(cd "$here/../lib" && pwd -P):$PATH"
ROUNDS=${ROUNDS:-3}
BASE=${BASE:-https://namios.dev}
CHANNEL=${CHANNEL:-nixos-unstable}
NIX_IMAGE=${NIX_IMAGE:-docker.io/nixos/nix:latest}
UBUNTU_IMAGE=${UBUNTU_IMAGE:-docker.io/library/ubuntu:24.04}

work=$(mktemp -d "${TMPDIR:-/tmp}/nami-bench.XXXXXX")
if [ "${KEEP:-}" = 1 ]; then echo "work folder: $work"; else trap 'rm -rf "$work"' EXIT INT TERM; fi
mkdir -p "$work/b" "$work/out"

# The key releases and catalogs are signed with, as the installer trusts it,
# and the catalog's nixpkgs revision (Nix is pinned to the same).
key=$(curl -fsSL "$BASE/install?via=test" | sed -n 's/^KEY="\([^"]*\)".*/\1/p')
[ -n "$key" ] || { echo "can't read the signing key from $BASE/install" >&2; exit 1; }
catalog=$(curl -fsSL "$BASE/menu/$CHANNEL?via=test")
nixpkgs=$(curl -fsSL "$BASE/menu/$catalog/git-revision")
case $nixpkgs in *[!0-9a-f]*|"") echo "unexpected nixpkgs revision: $nixpkgs" >&2; exit 1 ;; esac

# nami: the binary given, or the newest release, checked.
if [ $# -gt 0 ]; then
  cp "$1" "$work/nami"
else
  version=$(curl -fsSL "$BASE/releases/latest?via=test")
  tarball=nami-$version-x86_64-linux.tar.gz
  curl -fsSL -o "$work/$tarball" "$BASE/releases/$version/$tarball?via=test"
  curl -fsSL "$BASE/releases/$version/SHA256SUMS" | grep " \*\?$tarball\$" > "$work/SHA256SUMS"
  [ -s "$work/SHA256SUMS" ] || { echo "$tarball isn't in SHA256SUMS" >&2; exit 1; }
  (cd "$work" && sha256sum -c --quiet SHA256SUMS)
  tar xzf "$work/$tarball" -C "$work"
  cp "$work/nami-$version/nami" "$work/nami"
fi
chmod 755 "$work/nami"

# What runs in the containers. t LABEL cmd...: runs it, prints "T LABEL ms".
cat > "$work/b/t.sh" <<EOF
t() { l=\$1; shift; s=\$(date +%s%N); if "\$@" >/tmp/out-\$l 2>&1; then echo "T \$l \$(( (\$(date +%s%N) - s) / 1000000 ))"; else echo "FAILED \$l: \$*" >&2; tail -5 /tmp/out-\$l >&2; exit 1; fi; }
KEY='$key'
MENU='$BASE/menu/$CHANNEL'
R=github:NixOS/nixpkgs/$nixpkgs
export NIX_CONFIG="experimental-features = nix-command flakes"
EOF
cat > "$work/b/nami.sh" <<'EOF'
. /b/t.sh
nami --version >&2
t update nami update --trust "$KEY" --from "$MENU"
t install nami install ripgrep jq htop
t search nami search ripgrep
mkdir -p /p && cd /p && printf 'packages:\n  - bat\n' > project.nami
t run1 nami run -- bat --version
t run2 nami run -- bat --version
t run3 nami run -- bat --version
echo "S size $(stat -c %s /usr/local/bin/nami)"
EOF
cat > "$work/b/nix-install.sh" <<'EOF'
. /b/t.sh
nix --version >&2
t install nix profile add "$R#ripgrep" "$R#jq" "$R#htop"
EOF
cat > "$work/b/nix-rest.sh" <<'EOF'
. /b/t.sh
t search1 nix search "$R" ripgrep
t search nix search "$R" ripgrep
t run1 nix shell "$R#bat" -c bat --version
t run2 nix shell "$R#bat" -c bat --version
t run3 nix shell "$R#bat" -c bat --version
n=$(dirname "$(dirname "$(readlink -f "$(command -v nix)")")")
set -- $(nix path-info -S "$n")   # the image has no awk
echo "S size $2"
EOF

run() { # run <image> <script> [nami]: in a new container, results to stdout
  img=$1 script=$2; shift 2
  set -- -v "$work/b:/b:ro"
  [ "$img" = "$UBUNTU_IMAGE" ] && set -- "$@" -v "$work/nami:/usr/local/bin/nami:ro"
  docker run --rm "$@" "$img" sh "/b/$script"
}

echo "nixpkgs $nixpkgs (catalog $catalog), $ROUNDS rounds" >&2
i=1
while [ "$i" -le "$ROUNDS" ]; do
  echo "round $i of $ROUNDS" >&2
  for job in "nami $UBUNTU_IMAGE nami.sh" "nix $NIX_IMAGE nix-install.sh" "nix $NIX_IMAGE nix-rest.sh"; do
    set -- $job
    run "$2" "$3" > "$work/out/one" || { echo "$3 failed in round $i" >&2; exit 1; }
    sed "s/^/$1 /" "$work/out/one" >> "$work/out/all"
  done
  i=$((i + 1))
done

# The middle of the rounds' values for one measurement: mid <tool> <label>.
mid() {
  awk -v tool="$1" -v l="$2" '$1 == tool && $3 == l { print $4 }' "$work/out/all" | sort -n |
    awk '{ v[NR] = $1 } END { if (NR) print v[int((NR + 1) / 2)] }'
}
# nami's new computer is its catalog plus the install, taken per round.
awk '$1 == "nami" && ($3 == "update" || $3 == "install") { s += $4; if ($3 == "install") { print "nami T newpc", s; s = 0 } }' \
  "$work/out/all" >> "$work/out/all"

ms() { awk -v v="$1" 'BEGIN { if (v >= 1000) printf "%.1f s", v / 1000; else printf "%d ms", v }'; }
mb() { awk -v v="$1" 'BEGIN { printf "%.0f MB", v / 1000000 }'; }
row() { printf '%-34s %10s %10s\n' "$1" "$2" "$3"; }
echo
row "" nami Nix
row "Search all packages" "$(ms "$(mid nami search)")" "$(ms "$(mid nix search)")"
row "New computer, three tools" "$(ms "$(mid nami newpc)")" "$(ms "$(mid nix install)")"
row "Start a project's tools again" "$(ms "$(mid nami run3)")" "$(ms "$(mid nix run3)")"
row "What you install" "$(mb "$(mid nami size)")" "$(mb "$(mid nix size)")"
echo
echo "The middle of $ROUNDS rounds. Every value: $work/out/all$( [ "${KEEP:-}" = 1 ] || echo ' (KEEP=1 keeps it)')"
