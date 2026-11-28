#!/bin/bash
# Runs inside an Ubuntu container; see tests/completions.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null && apt-get install -y -qq bash-completion zsh fish python3 >/dev/null 2>&1

echo "== install.sh puts them where each shell looks"
/src/install.sh /new/nami >/dev/null
check "bash: bash-completion's folder" "grep -q '_nami()' /usr/local/share/bash-completion/completions/nami"
check "zsh: site-functions" "head -1 /usr/local/share/zsh/site-functions/_nami | grep -qx '#compdef nami'"
check "fish: its completions" "grep -q __nami_complete /etc/fish/completions/nami.fish"
check "nami completions zsh prints the same" "nami completions zsh | cmp - /usr/local/share/zsh/site-functions/_nami"

# A small catalog: a few apps and tools, and firefox's earlier versions.
mkdir -p /cat/versions
python3 - <<'PY'
import hashlib, json
p = lambda name, **kw: dict(name=name, outputs={"out": f"/nix/store/{hashlib.sha256(name.encode()).hexdigest()[:32]}-{name}"}, **kw)
cat = {
    "firefox": p("firefox-160.0", apps=["firefox.desktop"], summary="Browse the web", popularity=990),
    "firefox-esr": p("firefox-esr-140.5", apps=["firefox-esr.desktop"], summary="Browse the web, supported longer", popularity=400),
    "filezilla": p("filezilla-3.68", apps=["filezilla.desktop"], summary="Transfer files over FTP", popularity=300),
    "fish": p("fish-4.1", description="Smart and user-friendly command line shell"),
    "file": p("file-5.45", description="A program that shows the type of files"),
    "hello": p("hello-2.12.1", description="A program that produces a familiar, friendly greeting"),
    "jq": p("jq-1.7.1", description="Lightweight and flexible command-line JSON processor"),
}
json.dump(cat, open("/cat/index.json", "w"))
shard = hashlib.sha256(b"firefox").hexdigest()[:3]
old = lambda v, d: [v, d, "0" * 40, {"out": f"/nix/store/{'1' * 32}-firefox-{v}"}]
json.dump({"firefox": [old("159.0.2", "2026-11-12"), old("159.0", "2026-10-28"), old("158.0", "2026-10-01")]}, open(f"/cat/versions/{shard}.json", "w"))
PY
nami update --from /cat/index.json --unsigned >/dev/null 2>&1

useradd -m -s /bin/zsh ada
su - ada -c 'mkdir -p ~/app && echo "autoload -Uz compinit && compinit" > ~/.zshrc'
cat > /home/ada/app/project.nami <<'YAML'
tasks:
  test: cargo test
  lint: { run: cargo clippy, description: Checks the style }
services:
  - worker:
      about: Does the work
      run: sleep 1000
YAML
chown -R ada: /home/ada/app

echo "== how long nami takes to answer (a small catalog; ada, no daemon)"
su - ada -c 'cd ~/app && python3 /src/tests/lib/complete-timing.py nami' | sed 's/^/  /'

lines=("nami inst" "nami install fire" "nami install firefox@" "nami run ")
for sh in bash zsh fish; do
  su - ada -c "cd ~/app && python3 /src/tests/lib/complete-pty.py $sh $(printf "'%s' " "${lines[@]}")" > /tmp/$sh.out 2>&1
done
# What each line showed, from its === to the next.
shown() { awk -v want="=== $2" '$0 == want { on = 1; next } /^=== / { on = 0 } on' /tmp/$1.out; }

for sh in bash zsh fish; do
  echo "== $sh"
  check "nami inst<Tab>: install, install-to" "shown $sh 'nami inst' | grep -q 'install-to'"
  check "nami install fire<Tab>: the catalog's packages" "shown $sh 'nami install fire' | grep -q 'firefox-esr'"
  check "nami install firefox@<Tab>: its versions" "shown $sh 'nami install firefox@' | grep -q 'firefox@159' && shown $sh 'nami install firefox@' | grep -q 'firefox@158'"
  check "nami run <Tab>: the project's tasks" "shown $sh 'nami run ' | grep -q lint && shown $sh 'nami run ' | grep -q test"
done
echo "== with what each is (zsh, fish)"
for sh in zsh fish; do
  check "$sh: a command's short line" "shown $sh 'nami inst' | grep -q 'Install apps'"
  check "$sh: a package's summary" "shown $sh 'nami install fire' | grep -q 'Browse the web'"
  check "$sh: a version's newest release" "shown $sh 'nami install firefox@' | grep -q '159.0.2'"
  check "$sh: a task's description" "shown $sh 'nami run ' | grep -q 'Checks the style'"
done
check "fish's own engine agrees (complete -C)" "su - ada -c \"cd ~/app && fish -c 'complete -C \\\"nami install fire\\\"'\" | grep -q '^firefox-esr'"

echo "== a zsh session"
sed 's/^/  | /' /tmp/zsh.out

echo "== uninstall.sh takes them away"
sh /src/uninstall.sh >/dev/null 2>&1
check "none left" "[ ! -e /usr/local/share/bash-completion/completions/nami ] && [ ! -e /usr/local/share/zsh/site-functions/_nami ] && [ ! -e /etc/fish/completions/nami.fish ]"

if [ "$fails" -gt 0 ]; then
  echo "$fails failed"
  for sh in bash zsh fish; do echo "--- $sh"; cat /tmp/$sh.out; done
  exit 1
fi
echo "all passed"
