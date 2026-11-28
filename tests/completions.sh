#!/usr/bin/env sh
# Tab completion in bash (with bash-completion), zsh (compinit) and fish, as
# a person gets it: install.sh puts each shell's script where it looks, and
# typing `nami inst<Tab>`, `nami install fire<Tab>`, `nami install
# firefox@<Tab>` and `nami run <Tab>` in a pseudo-terminal shows what fits,
# from a small catalog and a test project. Also how long nami takes to
# answer, and that uninstall.sh takes the scripts away again.
# usage: tests/completions.sh   (needs target/release/nami: ./build.sh)
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
docker run --rm -v "$PWD":/src:ro -v "$PWD/target/release/nami":/new/nami:ro ubuntu:24.04 bash /src/tests/completions-inner.sh
