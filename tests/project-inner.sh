#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/project.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null 2>&1
nami daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash alice
as_alice() { su - alice -c "$1"; }

as_alice 'mkdir -p ~/app/tools/greet ~/app/src && cd ~/app && cat > project.nami' <<'YAML'
# Tools for this project only
packages:
  - jq
  - hello
  - ./tools/greet
env:
  GREETING: hej
YAML
as_alice 'cd ~/app && printf "name: greet\nversion: \"1\"\ninstall: |\n  mkdir -p \$out/bin\n  printf \"#!/bin/sh\\\\necho greetings from the repo\\\\n\" > \$out/bin/greet\n  chmod +x \$out/bin/greet\n" > tools/greet/package.nami'

echo "== tools only inside the project"
check "nami run: the project's jq" "as_alice 'cd ~/app/src && echo {\\\"a\\\":1} | nami run -- jq .a' | grep -qx 1"
check "its own recipe too" "as_alice 'cd ~/app && nami run -- greet' | grep -q 'greetings from the repo'"
check "and its env" "as_alice 'cd ~/app && nami run -- sh -c \"echo \\\$GREETING\"' | grep -qx hej"
check "outside the project, none of it" "! as_alice 'which jq' && ! as_alice 'which greet'"
check "nothing went into alice's profile" "! as_alice 'nami list' | grep -q jq"
check "nami shell: a shell with them" "as_alice 'cd ~/app && echo \"jq --version; hello --version | head -1\" | nami shell' | grep -q 'jq-'"
check "nami env, for direnv" "as_alice 'cd ~/app && nami env' | grep -q \"^export PATH='/nix/store/.*jq\""
check "and for fish" "as_alice 'cd ~/app && nami env --fish' | grep -q '^set -gx PATH '"

echo "== pinned, like flake.lock"
check "nami.lock pins every tool" "grep -q '\"jq\"' /home/alice/app/nami.lock && grep -q '\"./tools/greet\"' /home/alice/app/nami.lock && grep -q sha256 /home/alice/app/nami.lock"
check "--locked works from the lock" "as_alice 'cd ~/app && nami run --locked -- hello' | grep -q 'Hello, world'"
as_alice 'cd ~/app && sed -i "s/  - hello/  - hello\n  - ripgrep/" project.nami'
check "--locked refuses a project that changed" "! as_alice 'cd ~/app && nami run --locked -- hello' 2>&1 && as_alice 'cd ~/app && nami run --locked -- hello' 2>&1 | grep -q 'ripgrep isn.t in nami.lock'"
check "without it, the lock gains the new tool" "as_alice 'cd ~/app && nami run -- rg --version' | grep -q ripgrep && grep -q ripgrep /home/alice/app/nami.lock"

echo "== the lock pins the catalog too"
as_alice 'mkdir -p ~/pin && cd ~/pin && printf "packages:\n  - hello\n" > project.nami && nami run -- hello' >/dev/null 2>&1
nami update --from /newer.xz --unsigned >/dev/null 2>&1   # the computer moves on to a catalog without cowsay
check "the computer's catalog has no cowsay now" "! nami info cowsay --json 2>/dev/null | grep -q cowsay"
as_alice 'cd ~/pin && printf "packages:\n  - hello\n  - cowsay\n" > project.nami'
check "a tool added later comes from the project's own catalog" "as_alice 'cd ~/pin && nami run -- cowsay moo' | grep -q moo"
check "--upgrade moves the project to today's catalog (no cowsay there)" "as_alice 'cd ~/pin && nami run -- true && nami shell --upgrade </dev/null' 2>&1 | grep -q 'cowsay isn.t in the catalog'"
rm -f /nix/var/nami/menus/*; rm -rf /home/alice/.cache/nami/menus
as_alice 'cd ~/pin && git checkout nami.lock 2>/dev/null; true'
check "an old catalog that's gone is only fetched again if it's trusted" "as_alice 'cd ~/pin && printf \"packages:\\n  - hello\\n  - jq\\n\" > project.nami && nami run -- jq --version' 2>&1 | grep -q \"isn't available here\""

echo "== develop: a build environment, like nix develop"
as_alice 'mkdir -p ~/c && cd ~/c && printf "packages:\n  - openssl\ndevelop: true\n" > project.nami && cat > tls.c' <<'C'
#include <openssl/opensslv.h>
#include <openssl/crypto.h>
#include <stdio.h>
int main(void) { printf("built against %s\n", OpenSSL_version(OPENSSL_VERSION)); return 0; }
C
check "a C program builds against the project's openssl (pkg-config, headers, libs)" "as_alice 'cd ~/c && nami run -- sh -c \"cc tls.c \\\$(pkg-config --cflags --libs openssl) -o tls && ./tls\"' | grep -q 'built against OpenSSL'"
check "nami shell has the compiler" "as_alice 'cd ~/c && echo \"cc --version | head -1\" | nami shell' 2>/dev/null | grep -qi gcc"
check "nami env exports the build flags" "as_alice 'cd ~/c && nami env' | grep -q '^export NIX_CFLAGS_COMPILE='"
as_alice 'mkdir -p ~/z && cd ~/z && printf "packages:\n  - zlib\n" > project.nami && cat > z.c' <<'C'
#include <zlib.h>
#include <stdio.h>
int main(void) { printf("zlib %s\n", zlibVersion()); return 0; }
C
check "listing a library switches it on by itself, and says so" "as_alice 'cd ~/z && nami run -- sh -c \"cc z.c -lz -o z && ./z\"' > /tmp/z.out 2>&1; cat /tmp/z.out; grep -q 'build environment on (zlib is a library)' /tmp/z.out && grep -q '^zlib [0-9]' /tmp/z.out"
check "a project of plain tools stays a plain shell" "! as_alice 'cd ~/app && nami run -- true' 2>&1 | grep -q 'build environment'"

echo "== starters and tasks"
check "nami new lists the starters" "as_alice 'nami new' | grep -q '^  rust '"
check "nami new --from rust writes a project" "as_alice 'mkdir -p ~/r && cd ~/r && nami new --from rust' && grep -q cargo /home/alice/r/project.nami"
check "and won't overwrite one" "! as_alice 'cd ~/r && nami new --from go' 2>/dev/null && grep -q cargo /home/alice/r/project.nami"
as_alice 'mkdir -p ~/t && cd ~/t && cat > project.nami' <<'YAML'
packages:
  - hello
env:
  WHO: tasks
tasks:
  hi: hello -g "hi from $WHO"
  args: echo got
YAML
check "nami run <task> runs it with the project's tools and env" "as_alice 'cd ~/t && nami run hi' | grep -qx 'hi from tasks'"
check "extra arguments go to the task" "as_alice 'cd ~/t && nami run args one two' | grep -qx 'got one two'"
check "nami project tasks lists them" "as_alice 'cd ~/t && nami project tasks' | grep -q '^hi '"

echo "== nami hook: projects load when you cd in"
as_alice 'cat > hook-test.sh' <<'SH'
eval "$(nami hook bash)"
before="$PATH"
cd ~/t && _nami_hook 2>/dev/null
command -v hello >/dev/null && echo "in: hello" && echo "in: WHO=$WHO"
_nami_hook 2>/dev/null; echo "again: ${NAMI_HOOK_DIR:+same project}"
cd ~ && _nami_hook 2>/dev/null
command -v hello >/dev/null || echo "out: no hello"
[ "$PATH" = "$before" ] && echo "out: PATH as before"
[ -z "${WHO:-}" ] && echo "out: WHO gone"
SH
as_alice 'bash hook-test.sh' > /tmp/hook.out 2>&1
check "cd in: the project's tools and env" "grep -qx 'in: hello' /tmp/hook.out && grep -qx 'in: WHO=tasks' /tmp/hook.out"
check "staying: nothing reloads" "grep -qx 'again: same project' /tmp/hook.out"
check "cd out: tools gone, PATH exactly as before, env restored" "grep -qx 'out: no hello' /tmp/hook.out && grep -qx 'out: PATH as before' /tmp/hook.out && grep -qx 'out: WHO gone' /tmp/hook.out"
check "zsh and fish hooks too" "nami hook zsh | grep -q add-zsh-hook && nami hook fish | grep -q 'hook-env --fish'"

echo "== remote projects"
check "github:owner/repo@tag downloads that commit, and says when there's no project" "as_alice 'nami shell github:BurntSushi/ripgrep@14.1.1 </dev/null' 2>&1 | tee /dev/stderr | grep -q 'has no project.nami' && ls /home/alice/.cache/nami/remote/ | grep -q github-BurntSushi-ripgrep-"
check "bad references are refused before anything is downloaded" "as_alice 'nami shell github:../etc' 2>&1 | grep -q 'write it as github:owner/repo'"

echo "== tools from GitHub (public repos; a local mirror stands in for GitHub here)"
M=/srv/mirror
mk_commit() {  # mk_commit <sha> <greeting>: acme/tools at that commit
  rm -rf /tmp/src && mkdir -p /tmp/src/tools-$1/tools/greet
  printf "packages:\n  - hello\nprovides:\n  greet: ./tools/greet\n" > /tmp/src/tools-$1/project.nami
  printf "name: greet\nversion: \"1\"\ninstall: |\n  mkdir -p \$out/bin\n  printf '#!/bin/sh\\\\necho $2\\\\n' > \$out/bin/greet\n  chmod +x \$out/bin/greet\n" > /tmp/src/tools-$1/tools/greet/package.nami
  mkdir -p $M/acme/tools/refs && tar -czf $M/acme/tools/$1.tar.gz -C /tmp/src tools-$1
}
A=1111111111111111111111111111111111111111; B=2222222222222222222222222222222222222222
mk_commit $A "greetings, version one"; echo $A > $M/acme/tools/refs/HEAD; echo $A > $M/acme/tools/refs/v1
chmod -R a+rX $M
G="NAMI_GITHUB_MIRROR=$M"
check "nami install github:owner/repo#tool builds and installs what the repo provides" "as_alice '$G nami install github:acme/tools#greet' && as_alice greet | grep -qx 'greetings, version one'"
check "a tool the repo doesn't provide: says what it does provide" "as_alice '$G nami install github:acme/tools#nope' 2>&1 | grep -q 'provides greet, not \"nope\"'"
as_alice 'mkdir -p ~/spec && printf "packages:\n  - github:acme/tools@v1#greet:\n" > ~/spec/user.nami'
check "in a nami file, and the lock pins the commit" "as_alice '$G nami apply ~/spec/user.nami' && grep -q '\"rev\": \"$A\"' /home/alice/spec/nami.lock"
check "someone else's project, used remotely" "as_alice '$G nami run --project github:acme/tools -- hello' | grep -q 'Hello, world'"
mk_commit $B "greetings, version two"; echo $B > $M/acme/tools/refs/v1; chmod -R a+rX $M   # v1 moves on
useradd -m -s /bin/bash erin
mkdir -p /home/erin/spec && cp /home/alice/spec/user.nami /home/alice/spec/nami.lock /home/erin/spec/ && chown -R erin /home/erin/spec
check "with the lock, another user gets the pinned commit, not where the tag points now" "su - erin -c '$G nami apply --locked ~/spec/user.nami' >/dev/null 2>&1 && su - erin -c greet | grep -qx 'greetings, version one'"
check "--upgrade follows the tag" "as_alice '$G nami apply --upgrade ~/spec/user.nami' >/dev/null 2>&1 && as_alice greet | grep -qx 'greetings, version two' && grep -q '\"rev\": \"$B\"' /home/alice/spec/nami.lock"

echo "== dotfiles repos, and nami apply"
as_alice 'mkdir -p ~/dots && printf "packages:\n  - hello\naliases:\n  gs: git status\n" > ~/dots/user.nami'
check "a dotfiles repo's user.nami isn't a project" "as_alice 'cd ~/dots && nami run -- true' 2>&1 | grep -q 'no project.nami in /home/alice/dots or above'"
check "naming it says what it is" "as_alice 'nami shell ~/dots/user.nami </dev/null' 2>&1 | grep -q 'is a system or user file, not a project'"
as_alice 'cat > dots-hook.sh' <<'SH'
eval "$(nami hook bash)"
cd ~/dots && _nami_hook
echo "dir: ${NAMI_HOOK_DIR:-none}"
SH
check "and cd'ing in, the hook leaves it alone, silently" "as_alice 'bash dots-hook.sh' 2>&1 | grep -qx 'dir: none' && [ \"\$(as_alice 'bash dots-hook.sh' 2>&1)\" = 'dir: none' ]"
as_alice 'printf "packages: []\n" > ~/user.nami'
check "nami apply in a project applies your user file, not the project's" "as_alice 'cd ~/t && nami apply' && ! as_alice 'nami list' | grep -q hello"
check "and won't apply a project's file when named" "as_alice 'nami apply ~/t/project.nami' 2>&1 | grep -q \"is a project's file\""
check "a dotfiles repo's file applies when named" "as_alice 'nami apply ~/dots/user.nami' && as_alice 'nami list' | grep -q hello"

echo "== garbage collection"
jq_path=$(as_alice 'cd ~/app && nami run -- sh -c "command -v jq"')
check "nami gc keeps a project's tools" "nami gc >/dev/null 2>&1 && test -e \"$jq_path\""
as_alice 'rm -rf ~/app'
check "and frees them once the project is gone" "nami gc >/dev/null 2>&1 && ! test -e \"$jq_path\""

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; tail -5 /tmp/daemon.log; exit 1; }
