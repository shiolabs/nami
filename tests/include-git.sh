#!/bin/sh
# Sets from git repos: a computer's file builds on someone's published set
# (`github:alice/os/default.nami`), next to the namiOS example in its
# packages:; that set builds on another of her repo, which builds on bob's
# (`git+<url>@v2#dev/default.nami`). The first time, nami says what it
# brings and pins each repo's commit in nami.lock; `nami check`, `nami show`
# (each set's env) and the computer's plan see them; after the repos move
# on, the file still gets the pinned commits, offline too; `nami layers --update`
# moves the pins and says so; mistakes say what to do.
#
# No network, root, Docker or Nix needed: the repos are local bare repos,
# and github.com is pointed at them (git's url.<base>.insteadOf).
# usage: [NAMI_ROOT=<root with a catalog>] tests/include-git.sh
#        (work files: ${NAMI_INCLUDE_GIT_DIR:-~/.cache/nami-include-git-test})
set -eu
cd "$(dirname "$0")/.."
cargo build -q
T=$PWD/target/debug/nami
W=${NAMI_INCLUDE_GIT_DIR:-$HOME/.cache/nami-include-git-test}
rm -rf "${W:?}"
mkdir -p "$W/home" "$W/gh/alice" "$W/me"
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if sh -c "$2" >"$W/out" 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' "$W/out" | tail -15; fi; }
# Whoever runs it: a home of their own; github.com is the local repos.
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
cat > "$W/run.sh" <<EOF
#!/bin/sh
exec env HOME="$W/home" XDG_CACHE_HOME="$W/home/.cache" XDG_CONFIG_HOME= GIT_CONFIG_NOSYSTEM=1 ${NAMI_ROOT:+NAMI_ROOT="$NAMI_ROOT"} "$T" "\$@"
EOF
chmod +x "$W/run.sh"
R=$W/run.sh
G() { HOME="$W/home" GIT_CONFIG_NOSYSTEM=1 git "$@"; }

# alice/os: a default set (with another file of her repo), which builds
# on her gaming set, which builds on bob's.
G init -q --bare "$W/gh/alice/os.git"
G clone -q "$W/gh/alice/os.git" "$W/alice" 2>/dev/null
cat > "$W/alice/default.nami" <<'EOF'
# alice's computer: her tools.
packages: [./tools.nami, htop]
env:
  ALICE_LAYER: alice-one
EOF
cat > "$W/alice/tools.nami" <<'EOF'
packages: [github:alice/os/gaming.nami]
env:
  ALICE_TOOLS: tools
EOF
cat > "$W/alice/gaming.nami" <<EOF
packages: [jq, "git+file://$W/bob.git@v2#dev/default.nami"]
env:
  ALICE_GAMING: gaming
EOF
G -C "$W/alice" add -A
G -C "$W/alice" commit -qm one
G -C "$W/alice" push -q origin HEAD:main
ALICE1=$(G -C "$W/alice" rev-parse HEAD)

# bob/os: elsewhere (any git URL), a set in a folder, a tag v2.
G init -q --bare "$W/bob.git"
G clone -q "$W/bob.git" "$W/bob" 2>/dev/null
mkdir -p "$W/bob/dev"
cat > "$W/bob/dev/default.nami" <<'EOF'
packages: [ripgrep]
env:
  BOB_DEV: bob-v2
EOF
G -C "$W/bob" add -A
G -C "$W/bob" commit -qm two
G -C "$W/bob" tag v2
G -C "$W/bob" push -q origin HEAD:main v2
BOB=$(G -C "$W/bob" rev-parse HEAD)

S=$W/me/system.nami
# namiOS barebones; niri where namiOS's own packages aren't (nami's public repo).
COMPUTER=namios-bare; [ -d examples/packages/$COMPUTER ] || COMPUTER=niri
cat > "$S" <<EOF
packages: [github:alice/os/default.nami, $PWD/examples/packages/$COMPUTER]
hostname: laptop
users:
  me: { admin: true, password: "\$6\$x\$y" }
EOF

echo "== the first time: said, and pinned"
check "nami check passes" "$R check $S > $W/check1 2>&1"
check "it says what it includes: each repo and commit" "grep -q 'including github:alice/os (https://github.com/alice/os.git, its default branch) at commit $(echo $ALICE1 | cut -c1-12)' $W/check1 && grep -q \"including git+file://$W/bob.git@v2 .*v2) at commit $(echo $BOB | cut -c1-12)\" $W/check1"
check "nami.lock pins both repos (one pin for alice's two layers)" "jq -e '.layers | length == 2' $W/me/nami.lock && jq -e --arg r $ALICE1 '.layers[\"github:alice/os\"].rev == \$r' $W/me/nami.lock && jq -e --arg r $BOB '.layers[\"git+file://$W/bob.git@v2\"] | .rev == \$r and .ref == \"v2\"' $W/me/nami.lock"
check "the computer has every set: alice's (and her repo's other file), bob's, the namiOS example's" "$R show --flat $S > $W/show && grep -q 'ALICE_LAYER: alice-one' $W/show && grep -q 'ALICE_TOOLS: tools' $W/show && grep -q 'ALICE_GAMING: gaming' $W/show && grep -q 'BOB_DEV: bob-v2' $W/show && $R os-checks $S | grep -q '^updates by themselves'"
check "the second time: nothing said, nothing fetched" "$R check $S > $W/check2 2>&1 && ! grep -q including $W/check2"
check "nami layers lists them as written, at their commits" "$R layers $S > $W/layers && grep -q \"^github:alice/os/default.nami  $(echo $ALICE1 | cut -c1-12)  https://github.com/alice/os.git\" $W/layers && grep -q \"^github:alice/os/gaming.nami  $(echo $ALICE1 | cut -c1-12)\" $W/layers && grep -q \"@v2#dev/default.nami  $(echo $BOB | cut -c1-12)\" $W/layers"
check "the earlier spelling (the ref after the file) still works, and says how it's written now" "printf 'packages: [\"git+file://$W/bob.git#dev/default.nami@v2\"]\n' > $W/me/old.nami; $R check $W/me/old.nami > $W/old 2>&1 && grep -q \"write git+file://$W/bob.git@v2#dev/default.nami\" $W/old"

echo "== the repos move on; the file doesn't, until asked"
sed -i 's/alice-one/alice-two/g' "$W/alice/default.nami"
G -C "$W/alice" commit -qam two
G -C "$W/alice" push -q origin HEAD:main
ALICE2=$(G -C "$W/alice" rev-parse HEAD)
check "still alice's first commit" "$R show --flat $S | grep -q alice-one && jq -e --arg r $ALICE1 '.layers[\"github:alice/os\"].rev == \$r' $W/me/nami.lock"
mv "$W/gh/alice/os.git" "$W/gh/alice/os.away"
check "offline (the repo unreachable): pinned and fetched, it still works" "$R check $S && $R show --flat $S | grep -q alice-one"
check "a set not pinned yet, offline: it says it can't reach the repo" "printf 'packages: [github:alice/os/default.nami@main]\n' > $W/me/new.nami; ! $R layers $W/me/new.nami > $W/new 2>&1 && grep -q \"can't reach https://github.com/alice/os.git\" $W/new"
mv "$W/gh/alice/os.away" "$W/gh/alice/os.git"
check "nami layers --update moves the pin, and says so" "$R layers --update $S > $W/upd 2>&1 && grep -q \"github:alice/os: moved from $(echo $ALICE1 | cut -c1-12) to $(echo $ALICE2 | cut -c1-12)\" $W/upd && jq -e --arg r $ALICE2 '.layers[\"github:alice/os\"].rev == \$r' $W/me/nami.lock"
check "now the computer gets alice's second commit; bob's tag stays" "$R show --flat $S > $W/plan2 && grep -q alice-two $W/plan2 && grep -q bob-v2 $W/plan2 && jq -e --arg r $BOB '.layers[\"git+file://$W/bob.git@v2\"].rev == \$r' $W/me/nami.lock"
check "the pinned commits' files are kept, no git history in them" "test -f $W/home/.cache/nami/layers/github.com_alice_os-$(echo $ALICE1 | cut -c1-12)/default.nami && test -f $W/home/.cache/nami/layers/github.com_alice_os-$(echo $ALICE2 | cut -c1-12)/gaming.nami && ! test -e $W/home/.cache/nami/layers/github.com_alice_os-$(echo $ALICE2 | cut -c1-12)/.git"
check "a pinned commit that isn't here, and can't be fetched: it says to connect" "rm -rf $W/home/.cache/nami/layers && mv $W/gh/alice/os.git $W/gh/alice/os.away && ! $R check $S > $W/gone 2>&1; mv $W/gh/alice/os.away $W/gh/alice/os.git; grep -q 'connect to the network and run this again' $W/gone"

echo "== mistakes say what to do"
printf 'packages: [github:alice/os/racing.nami]\n' > "$W/me/racing.nami"
check "no such set: it lists the repo's" "! $R check $W/me/racing.nami > $W/e1 2>&1 && grep -q 'github:alice/os has no racing.nami' $W/e1 && grep -q 'its layers: default, gaming, tools' $W/e1"
printf 'packages: [github:alice/nothing/default.nami]\n' > "$W/me/nothing.nami"
check "no such repo: it says it can't reach it, and why it might be" "! $R check $W/me/nothing.nami > $W/e2 2>&1 && grep -q \"can't reach https://github.com/alice/nothing.git\" $W/e2 && grep -q 'public' $W/e2"
printf 'packages: [github:alice/os/default.nami@v9]\n' > "$W/me/v9.nami"
check "no such tag" "! $R check $W/me/v9.nami > $W/e3 2>&1 && grep -q 'has no branch or tag v9' $W/e3"
printf 'packages: [gitlab:alice/os/default.nami]\n' > "$W/me/form.nami"
check "an unknown form: it shows the ones there are" "! $R check $W/me/form.nami > $W/e4 2>&1 && grep -q 'github:owner/repo/<name>.nami' $W/e4"

if [ "$fails" -eq 0 ]; then echo "include-git: all passed"; else echo "include-git: $fails failed"; exit 1; fi
