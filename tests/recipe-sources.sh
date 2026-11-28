#!/bin/sh
# Recipes for source code, for real: `nami recipe from-url` writes each
# project's recipe (its kind, dependencies and sha256 worked out), `nami
# build` builds it with no edits, and its program runs. Then what a failed
# build says, a source that changed upstream, `nami recipe update` and
# `nami recipe new`.
#
#   hello      autotools, GNU's release tarball
#   htop       autotools, its release's own tarball; ncurses found from
#              shell code of its own
#   procps     GitLab; its release's tarball on SourceForge
#   cmark      CMake
#   cmatrix    CMake, written for CMake before 3.5; Curses
#   tio        Meson; glib, and Lua from a foreach of names
#   ripgrep    Cargo (pcre2-sys)
#   hexyl      Cargo
#   fzf        Go modules
#   shfmt      Go, several programs in one module
#   black      Python, hatchling and hatch-vcs; six dependencies
#   cowsay     Python, setuptools
#   marked     Node.js, npm, a build step
#
# Each at a version of its own (--version), so the run stays the same; one
# more finds hexyl's newest release.
#
# No root, Docker or Nix needed: nami runs as root of a user namespace with
# a scratch store mounted at /nix/store, through a copy of its loader, and
# the tools this script uses come from that store too (so it runs on a
# computer whose own tools are in /nix/store, like namiOS). Builds run in a
# memory-capped scope (8 GB) when there's a systemd user session.
# usage: tests/recipe-sources.sh [name...]   (only those projects)
#   work files: ${NAMI_RECIPE_DIR:-~/.cache/nami-recipe-test}; what it
#   downloads from the binary cache is kept, what it builds goes.
set -eu
# A test: namios.dev counts its update checks and downloads apart (NAMI_TEST, site/worker.js).
export NAMI_TEST=1
cd "$(dirname "$0")/.."
W=${NAMI_RECIPE_DIR:-$HOME/.cache/nami-recipe-test}
CHANNEL=${NAMI_CHANNEL:-https://namios.dev/menu/nixos-unstable}
KEY=${NAMI_KEY:-$(grep '^tray-3:' packaging/keys)}

if [ "${1:-}" != inner ]; then
  cargo build -q
  mkdir -p "$W/root/nix/store" "$W/lib" "$W/tools"
  for lib in $(ldd target/debug/nami | grep -o '/nix/store/[^ ]*'); do cp -Lf "$lib" "$W/lib/"; done
  if systemctl --user show -p Version >/dev/null 2>&1; then
    exec systemd-run --user --scope --quiet --collect -p MemoryMax=8G -p CPUWeight=20 -- nice -n 10 unshare -rm sh "$0" inner "$@"
  fi
  exec unshare -rm sh "$0" inner "$@"
fi
shift

R=$W/root
mount --bind "$R/nix/store" /nix/store
T=$PWD/target/debug/nami
nami() { NAMI_ROOT=$R "$W/lib/ld-linux-x86-64.so.2" --library-path "$W/lib" "$T" "$@"; }
[ -f "$R/nix/var/nami/index.json" ] || nami update --from "$CHANNEL" --trust "$KEY" >/dev/null
# The tools from here on: the scratch store's (the computer's may be in the /nix/store it hides).
printf 'packages: [coreutils, gnugrep, gnused, findutils, diffutils, gnutar, gzip]\n' > "$W/tools/project.nami"
eval "$(nami env --project "$W/tools" 2>"$W/tools.log")" || { cat "$W/tools.log"; exit 1; }

fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if (eval "$2") >"$W/out" 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' "$W/out" | tail -20; fi; }
wanted() { [ -z "$only" ] || case " $only " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
only=${*:-}

REPO=$PWD
rm -rf "${W:?}/work"
mkdir -p "$W/work"
cd "$W/work"

# from-url, build, run: name, URL, version, program and what its --version says.
project() {
  name=$1 url=$2 version=$3 run=$4 says=$5
  wanted "$name" || return 0
  check "$name: from-url writes its recipe" "nami recipe from-url $url --version $version --out $name >$name.recipe 2>&1 && grep -q '^version: \"*$version\"*$' $name/package.nami && grep -q '\"sha256\": \"[0-9a-f]\{64\}\"' $name/nami.lock && ! grep -q 'NOT FOUND' $name/package.nami"
  check "$name: builds with no edits" "nami build ./$name >$name.out 2>$name.log || { tail -15 $name.log; false; }"
  check "$name: $run says $says" "\$(cat $name.out)/bin/$run 2>&1 | grep -q -- '$says'"
}

echo "== from-url, build, run"
project hello https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz 2.12.1 "hello --version" "GNU Hello) 2.12.1"
project htop https://github.com/htop-dev/htop 3.5.3 "htop --version" "htop 3.5.3"
project procps https://gitlab.com/procps-ng/procps 4.0.7 "ps --version" "procps-ng 4.0.7"
project cmark https://github.com/commonmark/cmark 0.31.2 "cmark --version" "cmark 0.31.2"
project cmatrix https://github.com/abishekvashok/cmatrix 2.0 "cmatrix -V" "CMatrix version 2.0"
project tio https://github.com/tio/tio 3.9 "tio --version" "tio 3.9"
project ripgrep https://github.com/BurntSushi/ripgrep 16.0.0 "rg --version" "ripgrep 16.0.0"
project hexyl https://github.com/sharkdp/hexyl 0.17.0 "hexyl --version" "hexyl 0.17.0"
project fzf https://github.com/junegunn/fzf 0.75.0 "fzf --version" "0.75.0"
project shfmt https://github.com/mvdan/sh 3.14.1 "shfmt --version" "devel"
project black https://github.com/psf/black 26.10.1 "black --version" "black, 26.10.1"
project cowsay https://github.com/VaasuDevanS/cowsay-python 6.1 "cowsay --version" "6.1"
project marked https://github.com/markedjs/marked 18.3.0 "marked --version" "18.3.0"

if [ -z "$only" ]; then
  echo "== what it found"
  check "ripgrep: pcre2 from pcre2-sys in Cargo.lock" 'grep -q "^  - pcre2 .*Cargo.lock: pcre2-sys" ripgrep/package.nami'
  check "tio: glib and the first Lua the catalog has" 'grep -q "^  - glib .*glib-2.0" tio/package.nami && grep -q "^  - lua5_4 .*lua-5.4" tio/package.nami'
  check "htop: its release tarball, with configure made" 'grep -q "url: https://github.com/htop-dev/htop/releases/download/3.5.3/htop-3.5.3.tar.xz" htop/package.nami && grep -q "^  - ncurses" htop/package.nami'
  check "black: its backend and its dependencies, the catalog's Python packages" 'grep -q "^  - python3Packages.hatchling" black/package.nami && grep -q "^  - python3Packages.click" black/package.nami'
  check "the newest release when no --version" 'nami recipe from-url https://github.com/sharkdp/hexyl --out hexyl-newest && v=$(sed -n "s/^version: //p" hexyl-newest/package.nami) && [ -n "$v" ] && grep -q "refs/tags/v$v.tar.gz" hexyl-newest/package.nami'
  check "a recipe there already isn't overwritten" '! nami recipe from-url https://github.com/sharkdp/hexyl --version 0.17.0 --out hexyl'

  echo "== a failed build, in plain words"
  sed '/^  - glib /d' tio/package.nami > tio.nami && mkdir -p tio-noglib && mv tio.nami tio-noglib/package.nami && cp tio/nami.lock tio-noglib/
  check "a missing library: the line to add" '! nami build ./tio-noglib 2>err && cat err && grep -q "add \`glib\` to deps" err && grep -q "^  | .*glib-2.0" err'
  mkdir -p man/src man/recipe
  printf 'all:\n\tscdoc < tool.1.scd > tool.1\ninstall:\n\tinstall -Dm644 tool.1 $(out)/share/man/man1/tool.1\n' > man/src/Makefile
  printf 'tool(1)\n\n# NAME\n\ntool\n' > man/src/tool.1.scd
  printf 'name: tool\nversion: "1"\nsource: { path: ../src }\n' > man/recipe/package.nami
  check "a missing command: the line to add" '! nami build ./man/recipe 2>err && cat err && grep -q "the build runs scdoc, which isn.t there: add \`scdoc\` to build-deps" err'
  check "a lock file out of date (http-server's): says so" 'nami recipe from-url https://github.com/http-party/http-server --version 14.1.1 --out http-server && ! nami build ./http-server 2>err && cat err && grep -q "package-lock.json is out of date with package.json" err'

  echo "== no hashes by hand, and none taken silently"
  sed -i 's/"sha256": "[0-9a-f]\{4\}/"sha256": "0000/' hello/nami.lock
  check "a file that changed upstream is refused, with how to take it" '! nami build ./hello 2>err && cat err && grep -q "isn.t the file it was" err && grep -q "nami recipe update ./hello --refetch" err'
  check "update --refetch records the file as it is now" 'nami recipe update ./hello --refetch | grep -q "sha256 (nami.lock): 0000" && nami build ./hello'

  echo "== nami recipe update"
  check "from an older release to a newer one" 'nami recipe from-url https://github.com/sharkdp/hexyl --version 0.15.0 --out hexyl-old && nami recipe update ./hexyl-old --version 0.16.0 >said && cat said && grep -q "version: 0.15.0 → 0.16.0" said && grep -q "source.url: .*v0.15.0.tar.gz → .*v0.16.0.tar.gz" said && grep -q "sha256 (nami.lock):" said && grep -q "v0.16.0.tar.gz" hexyl-old/nami.lock && ! grep -q "v0.15.0" hexyl-old/nami.lock'
  check "a plain tarball: the newest in its folder" 'nami recipe from-url https://ftp.gnu.org/gnu/hello/hello-2.10.tar.gz --out hello-old && nami recipe update ./hello-old >said && cat said && grep -q "version: 2.10 → " said'
  check "at the newest already: says so" 'nami recipe update ./hexyl-newest | grep -q "is at its newest release"'

  echo "== nami recipe new"
  mkdir -p mytool/src
  printf '[package]\nname = "mytool"\nversion = "0.3.0"\nedition = "2021"\n\n[dependencies]\nmemchr = "2"\n' > mytool/Cargo.toml
  printf 'version = 4\n\n[[package]]\nname = "memchr"\nversion = "2.7.4"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "78ca9ab1a0babb1e7d5695e3530886289c18cf2f87ec19a575a0abdce112e3a3"\n\n[[package]]\nname = "mytool"\nversion = "0.3.0"\ndependencies = [\n "memchr",\n]\n' > mytool/Cargo.lock
  printf 'fn main() { println!("mytool {} {:?}", env!("CARGO_PKG_VERSION"), memchr::memchr(b'"'"'l'"'"', b"hello")); }\n' > mytool/src/main.rs
  check "a folder: its recipe, built" 'nami recipe new ./mytool --out mytool-recipe && grep -q "^  path: ../mytool" mytool-recipe/package.nami && nami build ./mytool-recipe >mt.out && $(cat mt.out)/bin/mytool | grep -qx "mytool 0.3.0 Some(2)"'

  echo "== nami's examples, one per kind"
  for e in rust-hexyl go-fzf python-cowsay node-marked; do
    check "examples/packages/$e builds" "cp -r $REPO/examples/packages/$e . && nami build ./$e >$e.out"
  done
  check "and they run" '$(cat rust-hexyl.out)/bin/hexyl --version && $(cat go-fzf.out)/bin/fzf --version && $(cat python-cowsay.out)/bin/cowsay --version && $(cat node-marked.out)/bin/marked --version'
fi

# What this run built goes; what it downloaded from the binary cache stays,
# for the next run.
for e in "$R"/nix/var/nami/db/*.json; do
  if grep -q '"built":true' "$e"; then rm -rf "$R/nix/store/$(basename "$e" .json)" "$e"; fi
done
rm -rf "$R/nix/var/nami/built"

echo
[ "$fails" -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
