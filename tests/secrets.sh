#!/bin/sh
# Secrets written right in nami files, for real: a project.nami with plain
# `!secret` values, refused by the pre-commit hook, sealed in place (the
# rest of the file byte for byte), shown, edited in $EDITOR (only the
# changed line changes), used by `nami up` and `nami env`; someone else
# can't open them until `nami secret allow` (an age key, and an SSH key),
# and can't after `--remove`, which lists what to change.
#
# No root, Docker or Nix needed: three people are three home folders, and
# the project's service needs no packages.
# usage: tests/secrets.sh   (work files: ${NAMI_SECRETS_DIR:-~/.cache/nami-secrets-test})
set -eu
cd "$(dirname "$0")/.."
cargo build -q
T=$PWD/target/debug/nami
W=${NAMI_SECRETS_DIR:-$HOME/.cache/nami-secrets-test}
rm -rf "${W:?}"
mkdir -p "$W/root"
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if sh -c "$2" >"$W/out" 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' "$W/out" | tail -15; fi; }
P=$W/shop
mkdir -p "$P"
# Each person: a script that runs a command as them, in the project: their
# own home (and keys), git without the host's config.
for p in alice bob carol; do
  mkdir -p "$W/$p/run" && chmod 700 "$W/$p/run"
  cat > "$W/$p.sh" <<EOF
#!/bin/sh
cd "$P"
exec env HOME="$W/$p" XDG_CONFIG_HOME="$W/$p/.config" XDG_CACHE_HOME="$W/$p/.cache" XDG_RUNTIME_DIR="$W/$p/run" USER="$p" NAMI_ROOT="$W/root" GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=$p GIT_AUTHOR_EMAIL=$p@x GIT_COMMITTER_NAME=$p GIT_COMMITTER_EMAIL=$p@x PATH="$(dirname "$T"):\$PATH" "\$@"
EOF
  chmod +x "$W/$p.sh"
done
A=$W/alice.sh B=$W/bob.sh C=$W/carol.sh
# nami up and nami env without a daemon: as root of a user namespace.
AR="unshare -r $A" BR="unshare -r $B"

cat > "$P/project.nami" <<'EOF'
# The shop. Its keys are test-mode ones.
env:
  STRIPE_KEY: !secret sk_test_abc123   # from the dashboard
  PLAIN: hello
services:
  - worker:
      run: echo "worker sees $STRIPE_KEY / $DB_PASSWORD" > {state}/seen; sleep 300
      env: { DB_PASSWORD: !secret "pw with spaces", QUEUE: default }
EOF
cp "$P/project.nami" "$W/before.nami"
$A git init -q

echo "== written plain, refused by the hook"
check "secret check lists both, and fails" "if $A nami secret check project.nami 2>$W/err; then exit 1; fi; [ \$(grep -c 'isn.t sealed' $W/err) = 2 ]"
check "the hook installs" "$A nami secret hook && test -x $P/.git/hooks/pre-commit"
check "a commit with plain secrets is refused" "$A git add project.nami && ! $A git commit -qm one 2>$W/hook.err && grep -q 'env.STRIPE_KEY isn.t sealed' $W/hook.err"
check "nami refuses to use them plain" "! $AR nami env 2>&1 | grep -q sk_test && $AR nami env 2>&1 | grep -q 'nami secret seal'"

echo "== sealed in place"
check "seal makes alice's key and .nami-recipients" "$A nami secret seal && test -s $W/alice/.config/nami/age.key && grep -q '^alice age1' $P/.nami-recipients"
check "her key is hers alone" "[ \$(stat -c %a $W/alice/.config/nami/age.key) = 600 ]"
check "no plain value left" "! grep -e sk_test_abc123 -e 'pw with' $P/project.nami"
check "sealed values are one line each" "[ \$(grep -c '!secret \"YWdl' $P/project.nami) = 2 ] && [ \$(wc -l < $P/project.nami) = \$(wc -l < $W/before.nami) ]"
sed 's/!secret "[^"]*"/S/' "$P/project.nami" > "$W/after-masked"
sed 's/!secret sk_test_abc123/S/; s/!secret "pw with spaces"/S/' "$W/before.nami" > "$W/before-masked"
check "everything else byte for byte" "cmp $W/before-masked $W/after-masked"
check "the comments and order are kept" "head -1 $P/project.nami | grep -q '^# The shop' && grep -q '   # from the dashboard\$' $P/project.nami && grep -q ', QUEUE: default }\$' $P/project.nami"
check "sealing again changes nothing" "cp $P/project.nami $W/sealed.nami && $A nami secret seal project.nami && cmp $P/project.nami $W/sealed.nami"
check "the commit goes through now" "$A git add -A && $A git commit -qm one"

echo "== show and edit"
check "show prints one secret, exactly" "[ \"\$($A nami secret show project.nami env.STRIPE_KEY)\" = sk_test_abc123 ]"
check "the end of its name is enough" "[ \"\$($A nami secret show project.nami DB_PASSWORD)\" = 'pw with spaces' ]"
check "without a key: the list" "$A nami secret show project.nami | grep -qx services.worker.env.DB_PASSWORD"
cat > "$W/editor.sh" <<'EOF'
#!/bin/sh
# Changes one secret, and saves what the editor saw (to check it was in memory).
grep -q '"sk_test_abc123"' "$1" || exit 3
echo "$1" > "$(dirname "$0")/edited-path"
sed -i 's/sk_test_abc123/sk_test_new456/' "$1"
EOF
chmod +x "$W/editor.sh"
check "edit: \$EDITOR gets them opened" "EDITOR=$W/editor.sh $A nami secret edit project.nami"
check "the opened copy was in \$XDG_RUNTIME_DIR, and is gone" "grep -q '^$W/alice/run/' $W/edited-path && ! test -e \"\$(cat $W/edited-path)\""
check "it's the new value now" "[ \"\$($A nami secret show project.nami STRIPE_KEY)\" = sk_test_new456 ]"
check "only its line changed" "[ \"\$($A git diff --numstat project.nami | cut -f1,2)\" = \"\$(printf '1\t1')\" ] && ! grep -q sk_test $P/project.nami"
check "an editor that fails changes nothing" "cp $P/project.nami $W/x && ! EDITOR=false $A nami secret edit project.nami && cmp $P/project.nami $W/x"
$A git commit -qam two

echo "== used"
check "nami env has them" "$AR nami env | grep -qx \"export STRIPE_KEY='sk_test_new456'\" && $AR nami env | grep -qx \"export PLAIN='hello'\""
check "nami up: the service gets its own and the project's" "$AR nami up -d && timeout 20 sh -c 'until [ -s $P/.nami/worker/seen ]; do sleep 0.2; done' && grep -qx 'worker sees sk_test_new456 / pw with spaces' $P/.nami/worker/seen"
check "nami down" "$AR nami down"
check "nothing opened on disk (but what the service wrote)" "! grep -rqs -e sk_test_new456 -e 'pw with spaces' $W/root $W/alice/.cache $W/alice/run $P/.git"

echo "== someone else"
check "bob can't open them, and is told how to get access" "! $B nami secret show project.nami STRIPE_KEY 2>$W/err; grep -q 'project.nami:3: can.t open the secret env.STRIPE_KEY' $W/err && grep -q 'nami secret allow' $W/err"
check "nor nami up" "! $BR nami up -d 2>$W/err; grep -q 'env.STRIPE_KEY: can.t open the secret' $W/err"
BOB=$($B nami secret key 2>/dev/null)
check "bob's key: an age key, made for him" "echo $BOB | grep -q '^age1' && test -s $W/bob/.config/nami/age.key"
check "alice allows bob: everything sealed again" "$A nami secret allow bob $BOB && grep -q '^bob age1' $P/.nami-recipients && ! $A git diff --quiet project.nami"
check "bob opens them" "[ \"\$($B nami secret show project.nami STRIPE_KEY)\" = sk_test_new456 ]"
if command -v ssh-keygen >/dev/null; then
  mkdir -p "$W/carol/.ssh"
  ssh-keygen -q -t ed25519 -N '' -C carol@x -f "$W/carol/.ssh/id_ed25519"
  check "carol's SSH key is her key" "[ \"\$($C nami secret key)\" = \"\$(cut -d' ' -f1,2 $W/carol/.ssh/id_ed25519.pub)\" ] && ! test -e $W/carol/.config/nami/age.key"
  check "alice allows it (from the .pub file)" "$A nami secret allow carol $W/carol/.ssh/id_ed25519.pub && grep -q '^carol ssh-ed25519 ' $P/.nami-recipients"
  check "carol opens them with her SSH key" "[ \"\$($C nami secret show project.nami DB_PASSWORD)\" = 'pw with spaces' ]"
else
  echo "  skip SSH keys (no ssh-keygen)"
fi
check "removing bob lists what to change" "$A nami secret allow bob --remove >$W/out2 2>&1; grep -q 'project.nami:3: env.STRIPE_KEY' $W/out2 && grep -q 'services.worker.env.DB_PASSWORD' $W/out2 && grep -q 'git keeps the old versions' $W/out2"
check "bob can't open them any more" "! $B nami secret show project.nami STRIPE_KEY 2>/dev/null && ! grep -q '^bob ' $P/.nami-recipients"
check "alice still can" "[ \"\$($A nami secret show project.nami STRIPE_KEY)\" = sk_test_new456 ]"
echo "== nami check: who can open them here, asking nothing"
check "alice can" "$A nami check project.nami </dev/null >$W/out2 2>&1; grep -q 'project.nami:3: note: env.STRIPE_KEY: you can open it here' $W/out2 && grep -q 'No problems' $W/out2"
check "bob can't, and is told how" "$B nami check project.nami </dev/null >$W/out2 2>&1; grep -q 'project.nami:3: warning: env.STRIPE_KEY: none of your keys here open it (tried $W/bob/.config/nami/age.key)' $W/out2 && grep -q 'nami secret allow' $W/out2"
if command -v ssh-keygen >/dev/null; then
  # dave's only key has a passphrase: nami check doesn't ask for it, it says so.
  D=$W/dave.sh
  sed 's/carol/dave/g' "$W/carol.sh" > "$D" && chmod +x "$D" && mkdir -p "$W/dave/.ssh" "$W/dave/run"
  ssh-keygen -q -t ed25519 -N 'dave pass' -C dave@x -f "$W/dave/.ssh/id_ed25519"
  $A nami secret allow dave "$W/dave/.ssh/id_ed25519.pub" >/dev/null 2>&1
  check "dave's key has a passphrase: not tried, and said so" "timeout 20 $D nami check project.nami </dev/null >$W/out2 2>&1; grep -q 'none of your keys here open it (you have no key here that needs no passphrase)' $W/out2 && grep -q \"note: couldn't try $W/dave/.ssh/id_ed25519 (it has a passphrase\" $W/out2"
  $A nami secret allow dave --remove >/dev/null 2>&1
fi
grep -q '^carol ' "$P/.nami-recipients" && $A nami secret allow carol --remove >/dev/null 2>&1
check "the last one can't be removed" "! $A nami secret allow alice --remove 2>$W/err && grep -q 'the last one' $W/err && grep -q '^alice ' $P/.nami-recipients"
check "the hook passes sealed files" "$A git add -A && $A git commit -qm three"

echo "== Wi-Fi networks in a system file (wifi:)"
# A config repo with a computer's file: its networks by name, one with a
# password. (tests/usb.sh boots one.)
S=$W/cfg/laptop
mkdir -p "$S"
$A git -C "$W/cfg" init -q
cat > "$S/system.nami" <<'EOF'
packages: [htop]
hostname: laptop
wifi:
  home: { password: !secret "home pass 1" }   # the router's
  cafe: {}
EOF
check "seal: the password, in place" "$A nami secret seal $S/system.nami && grep -q '^  home: { password: !secret \"YWdl.*\" }   # the router.s\$' $S/system.nami && grep -q '^alice age1' $W/cfg/.nami-recipients"
check "show: by its place, or its name" "[ \"\$($A nami secret show $S/system.nami wifi.home.password)\" = 'home pass 1' ] && [ \"\$($A nami secret show $S/system.nami home.password)\" = 'home pass 1' ]"
check "nami check: no problems, and alice can open it" "$A nami check $S/system.nami </dev/null >$W/out2 2>&1; grep -q 'system.nami:4: note: wifi.home.password: you can open it here' $W/out2 && grep -q 'No problems' $W/out2"
sed 's/  cafe: {}/  cafe: { pasword: x }/' "$S/system.nami" > "$S/typo.nami"
check "a typo in it: said, with its line" "! $A nami check $S/typo.nami >$W/out2 2>&1; grep -q 'typo.nami:3: error: wifi.cafe: pasword: did you mean password?' $W/out2"
rm -f "$S/typo.nami"
# The computer's own key, kept where nami usb keeps it.
LAPTOP=$(env HOME="$W/laptop" XDG_CONFIG_HOME="$W/laptop/.config" "$T" secret key 2>/dev/null)
mkdir -p "$W/alice/.cache/nami/os" && cp "$W/laptop/.config/nami/age.key" "$W/alice/.cache/nami/os/laptop.age.key"
check "nami check: the computer can't open it yet, and how it can" "$A nami check $S/system.nami >$W/out2 2>&1; grep -q 'warning: wifi.home.password: laptop (the computer) can.t open it (.nami secret allow laptop $LAPTOP. lets it' $W/out2"
check "allowed (in its repo): it can" "$A sh -c 'cd $W/cfg && nami secret allow laptop $LAPTOP' 2>/dev/null && $A nami check $S/system.nami >$W/out2 2>&1; grep -q 'note: wifi.home.password: laptop (the computer) can open it' $W/out2 && grep -q '^laptop age1' $W/cfg/.nami-recipients && ! grep -q '^laptop' $P/.nami-recipients"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
