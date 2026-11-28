#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/home.sh.
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

# alice already has a gitconfig of her own
as_alice 'printf "[user]\n\tname = Old Me\n" > ~/.gitconfig'
as_alice 'mkdir -p ~/dotfiles/nvim/lua && echo "vim.o.number = true" > ~/dotfiles/nvim/init.lua && echo "-- plugins" > ~/dotfiles/nvim/lua/plugins.lua'
as_alice 'cat > ~/user.nami' <<'EOF'
packages: [git, jq, python3]
env:
  EDITOR: nvim
  PAGER: less -R
aliases:
  ll: ls -la
  gs: git status
files:
  ~/.gitconfig:
    user: { name: Alice, email: alice@example.org }
    alias: { co: checkout, lg: "log --oneline --graph" }
    remote:
      origin: { url: "git@example.org:alice/dotfiles.git" }
  ~/.config/starship.toml:
    add_newline: false
    character: { success_symbol: "[➜](bold green)" }
  ~/.config/Code/User/settings.json:
    editor.fontSize: 14
    files.autoSave: afterDelay
  ~/.vimrc: |
    set number
    syntax on
  ~/.config/nvim: { source: ./dotfiles/nvim }
EOF

echo "== apply"
check "apply works" "as_alice 'nami apply ~/user.nami'"
grep -E '^\+ ~/|moved' /tmp/out | sed 's/^/       /'
check "her old gitconfig was kept as a backup" "as_alice 'grep \"Old Me\" ~/.gitconfig.nami-backup'"
check "git reads the generated config" "as_alice 'git config --global user.name' | grep -x Alice"
check "subsection [remote \"origin\"]" "as_alice 'git config --global remote.origin.url' | grep alice/dotfiles"
check "git alias works" "as_alice 'git config --global alias.lg' | grep -- --graph"
check "TOML parses (python tomllib)" "as_alice 'python3 -c \"import tomllib,os; print(tomllib.load(open(os.path.expanduser(\\\"~/.config/starship.toml\\\"),\\\"rb\\\"))[\\\"character\\\"][\\\"success_symbol\\\"])\"' | grep bold"
check "JSON parses (jq)" "as_alice 'jq -r .\\\"editor.fontSize\\\" ~/.config/Code/User/settings.json' | grep -x 14"
check "text file as written" "as_alice 'grep -x \"syntax on\" ~/.vimrc'"
check "directory from the repo" "as_alice 'cat ~/.config/nvim/lua/plugins.lua' | grep plugins"
check "env in login shells" "as_alice 'echo \$EDITOR' | grep -x nvim"
check "env in interactive bash" "su alice -c 'bash -ic \"echo \\\$PAGER\"' 2>/dev/null | grep -x 'less -R'"
check "aliases in interactive bash" "su alice -c 'bash -ic \"alias ll\"' 2>/dev/null | grep 'ls -la'"
check "env for the graphical session" "as_alice 'cat ~/.config/environment.d/60-nami-home.conf' | grep EDITOR=nvim"
check "fish config" "as_alice 'cat ~/.config/fish/conf.d/nami-home.fish' | grep 'alias gs'"

echo "== change it"
as_alice "sed -i 's/alice@example.org/alice@nami.dev/; /.vimrc: |/,/syntax on/d' ~/user.nami"
check "apply the change" "as_alice 'nami apply ~/user.nami'"
cp /tmp/out /tmp/apply.out
grep -E '~/' /tmp/apply.out | sed 's/^/       /'
check "diff names the changed file" "grep -- '~ ~/.gitconfig' /tmp/apply.out"
check "and the removed one" "grep -- '− ~/.vimrc' /tmp/apply.out"
check "new email in effect" "as_alice 'git config --global user.email' | grep -x alice@nami.dev"
check "removed file is gone" "as_alice '! test -e ~/.vimrc'"

echo "== rollback"
check "rollback" "as_alice 'nami rollback'"
check "old email back" "as_alice 'git config --global user.email' | grep -x alice@example.org"
check ".vimrc back" "as_alice 'grep -x \"syntax on\" ~/.vimrc'"

echo "== stop managing the gitconfig"
as_alice "nami rollback 2 >/dev/null 2>&1; sed -i '/.gitconfig:/,/url:/d' ~/user.nami"
check "apply without it" "as_alice 'nami apply ~/user.nami'"
check "her original gitconfig is restored" "as_alice 'git config --global user.name' | grep -x 'Old Me'"
check "no backup left behind" "as_alice '! test -e ~/.gitconfig.nami-backup'"

echo "== a user.nami without files: unlinks everything"
as_alice 'mkdir -p ~/bare && printf "packages: [git]\n" > ~/bare/user.nami'
check "apply" "as_alice 'nami apply ~/bare/user.nami'"
check "managed files are gone" "as_alice '! test -e ~/.config/starship.toml && ! test -e ~/.config/nvim'"
check "her own files stay" "as_alice 'test -d ~/dotfiles/nvim && grep Old ~/.gitconfig'"
check "the ~/.bashrc line is removed again" "as_alice '! grep -q nami/env.sh ~/.bashrc'"

echo "== mutable: apps may edit their own settings"
S='~/.config/Code/User/settings.json'
write_m() { as_alice "mkdir -p ~/m && printf 'packages: [jq]\nfiles:\n  ~/.config/Code/User/settings.json:\n    mutable: true\n    content: { editor.fontSize: $1 }\n' > ~/m/user.nami"; }
app_edits() { as_alice "jq '. + {\"workbench.colorTheme\": \"Dark\"}' $S > /tmp/s.json && cat /tmp/s.json > $S"; }
write_m 14
check "apply" "as_alice 'nami apply ~/m/user.nami'"
check "it's a real, writable file (not a link)" "as_alice 'test -f $S && ! test -L $S && test -w $S'"
app_edits
check "the app's edit worked" "as_alice 'jq -r .\\\"workbench.colorTheme\\\" $S' | grep -x Dark"
check "apply again (user.nami unchanged)" "as_alice 'nami apply ~/m/user.nami'"
check "the app's edit survives" "as_alice 'jq -r .\\\"workbench.colorTheme\\\" $S' | grep -x Dark"
write_m 16
check "change it in your nami file and apply" "as_alice 'nami apply ~/m/user.nami'"
grep -E 'moved' /tmp/out | sed 's/^/       /'
check "user.nami wins" "as_alice 'jq .\\\"editor.fontSize\\\" $S' | grep -x 16"
check "and the app's own edit is merged in, not lost" "as_alice 'jq -r .\\\"workbench.colorTheme\\\" $S' | grep -x Dark"
check "rollback (file untouched since) restores the old one" "as_alice 'nami rollback' && as_alice 'jq .\\\"editor.fontSize\\\" $S' | grep -x 14"
app_edits
as_alice "printf 'packages: [jq]\n' > ~/m/user.nami"
check "stop managing it after an edit" "as_alice 'nami apply ~/m/user.nami'"
cp /tmp/out /tmp/unmanage.out
check "says it's yours now" "grep 'yours now' /tmp/unmanage.out"
check "and leaves it in place" "as_alice 'jq -r .\\\"workbench.colorTheme\\\" $S' | grep -x Dark"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; tail -5 /tmp/daemon.log; exit 1; }
