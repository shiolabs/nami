#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/import.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null
nami daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash alice
as_alice() { su - alice -c "$1"; }
as_alice 'mkdir -p ~/nix && cp /src/tests/fixtures/import/e2e/home.nix ~/nix/'

echo "== import"
check "import writes her user file" "as_alice 'nami import ~/nix/home.nix'"
check "it has the shell setup" "as_alice 'grep -A3 \"shell:\" ~/user.nami' | grep -q zsh"
check "and nothing important left out" "! as_alice 'sed -n \"/Not converted/,\\\$p\" ~/user.nami' | grep -E 'programs\\.(zsh|starship|zoxide|tmux)'"

echo "== apply"
check "apply works" "as_alice 'nami apply'"
check "~/.zshrc made, sourcing nami's lines" "as_alice 'grep -q \"nami/shell.zsh\" ~/.zshrc'"
z() { as_alice "~/.local/state/nami/profile/bin/zsh -ic '$1' 2>/dev/null"; }
check "zsh comes from nami" "as_alice 'test -x ~/.local/state/nami/profile/bin/zsh'"
check "shell: ran (initContent)" "z 'echo \$NAMI_TEST_INIT' | grep -x yes"
check "oh-my-zsh loaded, from nami's share/" "z 'echo \$ZSH' | grep -q '/share/oh-my-zsh'"
check "its git plugin's aliases" "z 'alias gst' | grep -q 'git status'"
check "autosuggestions loaded" "z 'typeset -f _zsh_autosuggest_start >/dev/null && echo yes' | grep -x yes"
check "syntax highlighting loaded" "z 'echo \${ZSH_HIGHLIGHT_VERSION:-none}' | grep -qv none"
check "starship is the prompt" "z 'echo \$STARSHIP_SHELL' | grep -x zsh"
check "zoxide's z command" "z 'whence -w z' | grep -q function"
check "eza's aliases" "z 'alias ll' | grep -q eza"
check "home.shellAliases" "z 'alias gs' | grep -q 'git status'"
check "a variable using \$HOME" "z 'echo \$NOTES' | grep -x /home/alice/notes"
check "git config from programs.git" "as_alice 'git config --global user.name' | grep -x Alice"
check "tmux reads its config" "as_alice 'tmux -L t -f ~/.config/tmux/tmux.conf start-server \; show-options -g mouse \; kill-server' | grep -q 'mouse on'"
check "packages from home.packages" "as_alice 'rg --version && jq --version'"

echo "== undo"
check "undo works" "as_alice 'nami undo'"
check "the shell setup went with it" "! as_alice 'test -e ~/.config/nami/shell.zsh'"

# The same setup as a home-manager flake, on a computer without Nix: nami
# fetches Nix (through the daemon) to evaluate it.
echo "== a flake, without Nix"
as_alice 'cp -r /src/tests/fixtures/import/e2e/flake ~/flake'
check "there's no Nix here" "! command -v nix"
check "import reads it with Nix it fetched" "as_alice 'nami import ~/flake --to user --dry-run' 2>&1 | tee /tmp/flake.out | grep -q 'read with Nix'"
check "it said it was fetching Nix" "grep -q 'fetching it to read the flake' /tmp/flake.out"
check "the shell setup is there" "grep -q 'oh-my-zsh.sh' /tmp/flake.out"
check "home-manager's own files too" "grep -q 'tmux.conf' /tmp/flake.out"
check "a second import doesn't fetch Nix again" "! as_alice 'nami import ~/flake --to user --dry-run' 2>&1 | grep -q 'fetching it to read'"

# A new computer set up from a config repo, in one command. Ubuntu has no
# git here: nami uses the catalog's.
echo "== a config repo"
nami install git >/dev/null 2>&1
G=$(readlink -f /nix/var/nami/profiles/default/bin/git)
commit() { cd /srv/cfg && "$G" add -A && "$G" -c user.name=t -c user.email=t@t commit -qm "$1"; cd /; }
mkdir -p /srv/cfg/box/alice
printf 'packages:\n  - cowsay\nenv: { FROM_REPO: "yes" }\n' > /srv/cfg/box/alice/user.nami
printf 'packages:\n  - jq\n' > /srv/cfg/box/system.nami
"$G" init -q -b main /srv/cfg && commit init
nami remove git >/dev/null 2>&1
chmod -R a+rX /srv/cfg
# As on a new computer: no ~/user.nami of her own (the import above wrote one,
# and nami leaves a file of your own in place).
as_alice 'rm -f ~/user.nami'
# The repo is root's: git clones another user's local repo only when told it's safe.
safe="GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0='*'"
check "there's no git on the PATH" "! as_alice 'command -v git'"
check "alice sets up from the repo" "as_alice \"$safe nami apply file:///srv/cfg\" >/tmp/cfg.out 2>&1"
check "her ~/.config/nami is its checkout" "as_alice 'test -d ~/.config/nami/.git && grep -q cowsay ~/.config/nami/box/alice/user.nami && test \"\$(readlink ~/user.nami)\" = /home/alice/.config/nami/box/alice/user.nami'"
check "what was there is kept aside" "as_alice 'ls -d ~/.config/nami.before-*'"
check "its packages" "as_alice '~/.local/state/nami/profile/bin/cowsay hi'"
check "its environment" "as_alice 'bash -lc \"echo \\\$FROM_REPO\"' | grep -x yes"
check "the computer's file isn't hers to apply" "grep -q 'leaving out box/system.nami' /tmp/cfg.out"
check "the secrets key stays out of git" "as_alice 'grep -qx /age.key ~/.config/nami/.git/info/exclude'"
printf 'packages:\n  - cowsay\n  - hello\nenv: { FROM_REPO: "yes" }\n' > /srv/cfg/box/alice/user.nami && commit more
check "applying again pulls what changed" "as_alice \"$safe nami apply file:///srv/cfg\" && as_alice '~/.local/state/nami/profile/bin/hello'"
check "root applies the computer's side" "env $safe nami apply file:///srv/cfg >/tmp/cfg-root.out 2>&1 && grep -q jq /etc/nami/system.nami && test -x /nix/var/nami/profiles/default/bin/jq"
check "and leaves people's settings to them" "! grep -q 'your settings' /tmp/cfg-root.out"

echo
[ $fails -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
