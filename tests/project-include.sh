#!/bin/sh
# A project builds on a team's set: project.nami lists `./nami/common.nami`,
# which lists `github:acme/dev/node-service.nami` (one file each), and the
# layer's package, task and service are the project's (`nami run lint`,
# `nami up`), with the project's own values winning. The first time, nami
# says which commit it includes and pins it in the project's nami.lock; a
# teammate's clone gets that commit, after the layer moved on too;
# `nami layers --update` moves the pin and says from what to what;
# `--locked` refuses a layer that isn't pinned; offline, what's fetched
# still works; the cd hook (`nami hook-env`) never fetches, and says how to.
#
# No network, Docker or Nix needed. The layer's repo is a local bare repo
# that github.com points at (git's url.<base>.insteadOf). A package needs a
# store: nami runs as root of a user namespace, with a scratch store
# mounted at /nix/store holding `hello` (copied from a root that has the
# catalog and it, as tests/check-root makes), and runs through a copy of
# its loader and libraries, since the mount hides the host's store.
# usage: NAMI_ROOT=<root with a catalog and hello> tests/project-include.sh
#        (work files: ${NAMI_PROJECT_INCLUDE_DIR:-~/.cache/nami-project-include-test})
set -eu
cd "$(dirname "$0")/.."
W=${NAMI_PROJECT_INCLUDE_DIR:-$HOME/.cache/nami-project-include-test}

if [ "${1:-}" != inner ]; then
  CR=${NAMI_ROOT:?NAMI_ROOT: a root with a catalog and hello in its store}
  cargo build -q
  [ -d "$W" ] && chmod -R u+w "$W"   # the store copy is read-only
  rm -rf "${W:?}"
  mkdir -p "$W/root/nix/store" "$W/root/nix/var/nami/db" "$W/lib" "$W/home" "$W/gh/acme"
  for lib in $(ldd target/debug/nami | grep -o '/nix/store/[^ ]*'); do cp -Lf "$lib" "$W/lib/"; done
  # hello and what it needs, and a catalog that has it.
  python3 -I - "$CR" "$W/root" <<'EOF'
import json, os, shutil, sys
src, dst = sys.argv[1], sys.argv[2]
cat = json.load(open(f"{src}/nix/var/nami/index.json"))
json.dump({"hello": cat["hello"]}, open(f"{dst}/nix/var/nami/index.json", "w"))
todo, seen = [os.path.basename(cat["hello"]["outputs"]["out"])], set()
while todo:
    b = todo.pop()
    if b in seen:
        continue
    seen.add(b)
    info = f"{src}/nix/var/nami/db/{b}.json"
    shutil.copy(info, f"{dst}/nix/var/nami/db/")
    shutil.copytree(f"{src}/nix/store/{b}", f"{dst}/nix/store/{b}", symlinks=True)
    todo += json.load(open(info))["references"]
EOF
  exec unshare -rm sh "$0" inner
fi

# Root of a user namespace: the host's programs only (its /nix/store is
# hidden from here on).
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
mount --bind "$W/root/nix/store" /nix/store
T=$PWD/target/debug/nami
cat > "$W/home/.gitconfig" <<EOF
[user]
	name = t
	email = t@example.com
[init]
	defaultBranch = main
[commit]
	gpgsign = false
[url "file://$W/gh/"]
	insteadOf = https://github.com/
EOF
cat > "$W/nami" <<EOF
#!/bin/sh
exec env HOME="$W/home" XDG_CACHE_HOME="$W/home/.cache" XDG_CONFIG_HOME= GIT_CONFIG_NOSYSTEM=1 NAMI_ROOT="$W/root" "$W/lib/ld-linux-x86-64.so.2" --library-path "$W/lib" "$T" "\$@"
EOF
chmod +x "$W/nami"
R=$W/nami
G() { HOME="$W/home" GIT_CONFIG_NOSYSTEM=1 git "$@"; }
LAYERS=$W/root/nix/var/nami/layers   # root's fetched layers
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if sh -c "$2" >"$W/out" 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' "$W/out" | tail -15; fi; }
short() { echo "$1" | cut -c1-12; }

# acme/dev: the team's layers. node-service brings hello, a task, a
# service, and env; it lists base.nami, another file of its repo.
G init -q --bare "$W/gh/acme/dev.git"
G clone -q "$W/gh/acme/dev.git" "$W/acme" 2>/dev/null
mkdir -p "$W/acme/scripts"
cat > "$W/acme/node-service.nami" <<'EOF'
packages: [./base.nami, hello]
env:
  GREETING: from-the-layer
  LAYER_ONLY: "yes"
tasks:
  lint: hello -g "lint v1 $GREETING"
  test: echo layer-test
services:
  - worker:
      run: echo "worker $WHO"; exec sleep 600
      env: { WHO: layer }
EOF
cat > "$W/acme/base.nami" <<'EOF'
env:
  BASE: "1"
EOF
G -C "$W/acme" add -A
G -C "$W/acme" commit -qm one
G -C "$W/acme" push -q origin HEAD:main
ONE=$(G -C "$W/acme" rev-parse HEAD)

# The project: its own repo, with a file of its own it includes too.
P=$W/app
mkdir -p "$P/nami"
G init -q "$P"
cat > "$P/project.nami" <<'EOF'
packages: [./nami/common.nami]
env:
  GREETING: from-the-project
tasks:
  test: echo own-test
services:
  - worker:
      env: { WHO: project }
EOF
cat > "$P/nami/common.nami" <<'EOF'
packages: [github:acme/dev/node-service.nami]
env:
  COMMON: "1"
EOF
cd "$P"

echo "== the first time: said, and pinned in the project's nami.lock"
check "nami check passes" "$R check project.nami > $W/check1 2>&1"
check "it says what it includes: the repo and commit" "grep -q 'including github:acme/dev (https://github.com/acme/dev.git, its default branch) at commit $(short $ONE)' $W/check1 && grep -q 'runs code in the project' $W/check1"
check "nami.lock pins it" "grep -q '\"github:acme/dev\"' nami.lock && grep -q '\"rev\": \"$ONE\"' nami.lock"
check "nami run <task> from the layer: its package, and the project's env winning" "$R run lint > $W/lint 2>&1 && grep -q '^lint v1 from-the-project\$' $W/lint"
check "the project's own task wins" "$R run test 2>/dev/null | grep -qx own-test"
check "env from every layer: the layer's, its own include's, the project's file's" "$R env > $W/env 2>/dev/null && grep -q \"LAYER_ONLY='yes'\" $W/env && grep -q \"BASE='1'\" $W/env && grep -q \"COMMON='1'\" $W/env && grep -q \"GREETING='from-the-project'\" $W/env"
check "the second time: nothing said" "$R run test > $W/run2 2>&1 && ! grep -q including $W/run2"
check "the layer's service, with the project's change to it" "$R up -d > $W/up 2>&1 && $R project services | grep -q '^worker .*running' && for i in 1 2 3 4 5 6 7 8 9 10; do grep -q 'worker project' .nami/worker.log 2>/dev/null && break; sleep 0.5; done; grep -q 'worker project' .nami/worker.log; r=\$?; $R down >/dev/null 2>&1; exit \$r"
check "nami layers in the project lists it" "$R layers > $W/layers && grep -q \"^github:acme/dev/node-service.nami  $(short $ONE)  https://github.com/acme/dev.git\" $W/layers && grep -q \"pinned in $P/nami.lock\" $W/layers"
G add -A
G commit -qm app

echo "== the layer moves on; the project, and a teammate's clone, don't"
sed -i 's/lint v1/lint v2/' "$W/acme/node-service.nami"
G -C "$W/acme" commit -qam two
G -C "$W/acme" push -q origin HEAD:main
TWO=$(G -C "$W/acme" rev-parse HEAD)
G clone -q "$P" "$W/clone"
cd "$W/clone"
check "the clone gets the pinned commit" "$R run lint 2>&1 | grep -q '^lint v1 from-the-project\$' && grep -q '\"rev\": \"$ONE\"' nami.lock"
check "--locked: the pinned one is fine" "$R run --locked lint 2>&1 | grep -q '^lint v1'"
cp nami/common.nami common.nami.orig
sed -i 's|node-service.nami\]|node-service.nami@main]|' nami/common.nami
check "--locked refuses a layer that isn't pinned" "! $R run --locked lint > $W/locked 2>&1 && grep -q \"github:acme/dev@main isn't pinned in\" $W/locked && grep -q 'nami layers. (or .nami shell.)' $W/locked"
mv common.nami.orig nami/common.nami
mv "$W/gh/acme/dev.git" "$W/gh/acme/dev.away"
check "offline (the repo unreachable): what's fetched still works" "$R run lint 2>&1 | grep -q '^lint v1'"
mv "$W/gh/acme/dev.away" "$W/gh/acme/dev.git"
check "nami layers --update moves the pin, and says from what to what" "$R layers --update > $W/upd 2>&1 && grep -q \"github:acme/dev: moved from $(short $ONE) to $(short $TWO)\" $W/upd && grep -q '\"rev\": \"$TWO\"' nami.lock"
check "now the clone gets the second commit; the first project still the first" "$R run lint 2>&1 | grep -q '^lint v2' && cd $P && $R run lint 2>&1 | grep -q '^lint v1'"

echo "== the cd hook never fetches"
cd "$P"
check "pinned and fetched: it loads, from the cache" "$R hook-env > $W/hook1 2> $W/hook1.err && grep -q \"GREETING='from-the-project'\" $W/hook1 && grep -q 'project loaded' $W/hook1.err"
rm -rf "$LAYERS"
check "pinned, not fetched here: it says how, and fetches nothing" "$R hook-env > $W/hook2 2> $W/hook2.err; grep -q \"isn't fetched here yet: .nami layers. (or .nami shell.) in that folder fetches it\" $W/hook2.err && ! test -e $LAYERS/github.com_acme_dev-$(short $ONE)"
check "nami layers fetches it" "$R layers >/dev/null 2>&1 && test -f $LAYERS/github.com_acme_dev-$(short $ONE)/node-service.nami"
mkdir -p "$W/fresh"
printf 'packages: [github:acme/dev/node-service.nami]\n' > "$W/fresh/project.nami"
check "not pinned yet: it says how, and pins nothing" "cd $W/fresh && $R hook-env > $W/hook3 2> $W/hook3.err; grep -q \"isn't pinned in .* yet: .nami layers. (or .nami shell.)\" $W/hook3.err && ! test -e $W/fresh/nami.lock"

echo "== mistakes say what to do"
# namiOS barebones; niri where namiOS's own packages aren't (nami's public repo).
COMPUTER=namios-bare; [ -d examples/packages/$COMPUTER ] || COMPUTER=niri
printf 'packages: [%s/examples/packages/%s]\n' "$PWD" "$COMPUTER" > "$W/fresh/project.nami"
check "a computer's package isn't a project's" "cd $W/fresh && ! $R check project.nami > $W/e1 2>&1 && grep -q '$COMPUTER: a package for a computer' $W/e1"

if [ "$fails" -eq 0 ]; then echo "project-include: all passed"; else echo "project-include: $fails failed"; exit 1; fi
