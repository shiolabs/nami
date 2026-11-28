#!/bin/sh
# Moving from an old computer: `nami import` on an Ubuntu computer (a folder
# with its files: dpkg's and apt's state, snaps, flatpaks, NetworkManager's
# Wi-Fi networks, a home with dotfiles and toolchains, and GNOME settings
# from a faked `dconf dump`) and on Windows' disk writes a config repo. It
# passes `nami check`, no secret is in it in plain text (they open with the
# person's key), a second run changes nothing and keeps what you edited,
# and `--home` moves the files.
#
# No root, Docker or Nix needed. With a catalog (NAMI_ROOT pointing at a
# nami root that has one) the apps are checked against it too.
# usage: [NAMI_ROOT=<root with a catalog>] tests/import-computer.sh
#        (work files: ${NAMI_IMPORT_DIR:-~/.cache/nami-import-test})
set -eu
cd "$(dirname "$0")/.."
cargo build -q
T=$PWD/target/debug/nami
F=$PWD/tests/fixtures/import/computer
W=${NAMI_IMPORT_DIR:-$HOME/.cache/nami-import-test}
CATALOG=${NAMI_ROOT:-$W/troot}
rm -rf "${W:?}"
mkdir -p "$W/home" "$W/bin" "$W/troot"
cp -r "$F/ubuntu/root" "$W/ubuntu"
cp -r "$F/windows/root" "$W/windows"
# GNOME's settings: what `dconf dump /` printed on that computer.
printf '#!/bin/sh\ncat %s\n' "$F/ubuntu/dconf.txt" > "$W/bin/dconf"
chmod +x "$W/bin/dconf"
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if sh -c "$2" >"$W/out" 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' "$W/out" | tail -15; fi; }
# Whoever runs it: a home of their own (their secrets key is made there).
cat > "$W/run.sh" <<EOF
#!/bin/sh
exec env HOME="$W/home" USER=tester XDG_CONFIG_HOME= NAMI_ROOT="$CATALOG" PATH="$W/bin:\$PATH" "$T" "\$@"
EOF
chmod +x "$W/run.sh"
R=$W/run.sh
REPO=$W/repo
S=$REPO/oldlaptop/system.nami
U=$REPO/oldlaptop/alice/user.nami
has_catalog() { test -s "$CATALOG/nix/var/nami/index.json"; }
# The fixture's secrets (fake ones), which must never be in the repo plain.
PLAIN="-e 'correct horse battery' -e not-a-real-token -e fake-token-for-tests -e not-a-real-password -e garden-pass-123"

echo "== an Ubuntu computer"
check "two people there: it asks whose" "$R import $W/ubuntu --out $REPO 2>&1 | grep -q 'say whose settings to bring with --user'"
check "--dry-run shows both files, and writes nothing" "$R import $W/ubuntu --user alice --out $REPO --dry-run > $W/dry 2>$W/dry.err && grep -q '^# $S' $W/dry && grep -q '^# $U' $W/dry && ! test -e $REPO"
check "and shows no secret" "! grep -q $PLAIN $W/dry $W/dry.err && grep -q '<hidden>' $W/dry"
check "import writes the repo" "$R import $W/ubuntu --user alice --out $REPO > $W/import.out 2>&1 && test -s $S && test -s $U && test -s $REPO/.nami-recipients && test -s $REPO/.gitignore"
check "it says what came, what didn't, and what's next" "grep -q '^brought over:' $W/import.out && grep -q '^not brought over' $W/import.out && grep -q 'nami usb $REPO' $W/import.out && grep -q -- '--home' $W/import.out"
check "no secret in the repo, nor in what it printed" "! grep -rq $PLAIN $REPO $W/import.out"
check "the secrets key is the person's, outside the repo; one .nami-recipients" "test -s $W/home/.config/nami/age.key && grep -q '^tester age1' $REPO/.nami-recipients && ! test -e $REPO/age.key && ! test -e $REPO/oldlaptop/.nami-recipients"
check "nami check: the computer's file" "$R check $S"
check "nami check: the person's file" "$R check $U"
check "the Wi-Fi password opens with the key" "[ \"\$($R secret show $S wifi.Home.password)\" = 'correct horse battery' ]"
check "and the work network's (PEAP), with its identity" "[ \"\$($R secret show $S wifi.Campus.password)\" = not-a-real-password-2 ] && grep -A4 'Campus:' $S | grep -q 'security: enterprise' && grep -A4 'Campus:' $S | grep -q 'identity: alice'"
check "and the token from ~/.bashrc" "[ \"\$($R secret show $U GITHUB_TOKEN)\" = not-a-real-token-0000000000 ]"
check "~/.npmrc (with a token) is sealed whole" "$R secret show $U | grep -q '.npmrc'"
check "Wi-Fi as wifi: entries; the open network says so" "grep -q '^wifi:' $S && grep -A1 'Cafe Wifi:' $S | grep -q 'security: open' && ! grep -q nmconnection $S"
check "this computer's Wi-Fi card stays behind" "! grep -q -e mac-address -e interface-name -e seen-bssids $S"
check "the certificate network and the wired one: listed, not brought" "grep -q 'Wi-Fi network Corp' $S && ! grep -q 'Wired' $S"
check "the computer: namiOS, name, zone, language, keyboard, you" "grep -A1 '^packages:' $S | grep -q namios && grep -q 'hostname: oldlaptop' $S && grep -q 'timezone: Europe/Copenhagen' $S && grep -q 'language: da_DK.UTF-8' $S && grep -q 'keyboard: dk' $S && grep -A3 'alice:' $S | grep -q 'admin: true'"
check "apps from the table (code is vscode, fd-find is fd)" "grep -qx '  - vscode' $S && grep -qx '  - fd' $S && grep -qx '  - google-chrome' $S"
check "what the system brought stays out (bash, gnome-shell)" "! grep -qx -e '  - bash' -e '  - gnome-shell' -e '  - nautilus' $S"
check "libraries, the kernel, language packs: listed" "grep -q 'libssl-dev' $S && grep -q 'linux-generic' $S && grep -q 'language-pack-da' $S"
check "an apt package with no match: listed, with --recipes as the way" "grep -q 'apt package acme-vpn: not in the catalog (.--recipes' $S"
check "snaps: Ubuntu's own left out, acme-notes listed" "! grep -q -e core22 -e snap-store $S && grep -q 'snap acme-notes' $S"
if has_catalog; then
  check "apps checked against the catalog: btop, qemu, gimp (its menu entry), spotify (snap), gnome-boxes (flatpak)" "grep -qx '  - btop' $S && grep -qx '  - qemu' $S && grep -qx '  - gimp' $S && grep -qx '  - spotify' $S && grep -qx '  - gnome-boxes' $S"
  check "toolchains: rustup, nvm's node, pyenv's python, cargo-installed ripgrep" "grep -qx '  - rustup' $U && grep -qx '  - nodejs_22' $U && grep -qx '  - python312' $U && grep -qx '  - ripgrep' $U"
else
  echo "  (no catalog at \$NAMI_ROOT: the apps are matched by nami's table only)"
fi
check "GNOME: dark mode, keyboard layouts as GVariant, night light, a custom key" "grep -q 'color-scheme: prefer-dark' $U && grep -q \"sources: !gvariant\" $U && grep -q 'night-light-enabled: true' $U && grep -q 'custom-keybindings/custom0/' $U"
check "favourite apps in the dock, as nami names them" "grep -A6 'pinned:' $U | grep -q 'code' && ! grep -q -e snap-store -e firefox_firefox $U"
check "GNOME Shell's extensions and app state stay behind" "! grep -q -e enabled-extensions -e dash-to-dock -e window-state -e last-panel -e command-history $U"
check "the shell: variables, aliases, the rest as lines; not the secret one" "grep -q 'EDITOR: vim' $U && grep -q 'll: ls -alF --color' $U && grep -q 'starship init bash' $U && ! grep -q 'example.org/deploy' $U"
check "dotfiles: .gitconfig and .ssh/config, never a key" "grep -q '.gitconfig:' $U && grep -q '.ssh/config:' $U && ! grep -q 'id_ed25519:' $U"

echo "== again"
cp "$S" "$W/s1"; cp "$U" "$W/u1"
check "a second run changes nothing" "$R import $W/ubuntu --user alice --out $REPO > $W/again.out 2>&1 && cmp $S $W/s1 && cmp $U $W/u1 && grep -q 'nothing new' $W/again.out"
sed -i 's/^timezone: Europe\/Copenhagen/timezone: Europe\/Berlin   # moved/' "$S"
printf '# my own note\n' >> "$U"
check "what you changed stays, after another run" "$R import $W/ubuntu --user alice --out $REPO >/dev/null 2>&1 && grep -q 'timezone: Europe/Berlin   # moved' $S && ! grep -q 'Europe/Copenhagen' $S && grep -q '^# my own note' $U"
check "and the repo still checks" "$R check $S && $R check $U"

echo "== root only"
cp -r "$W/ubuntu" "$W/locked"
chmod 000 "$W/locked/etc/NetworkManager/system-connections"
if [ "$(id -u)" != 0 ]; then
  check "Wi-Fi it can't read: it says to use sudo" "$R import $W/locked --user alice --out $W/repo2 2>&1 | grep -q 'sudo nami import'"
fi
chmod 755 "$W/locked/etc/NetworkManager/system-connections"

echo "== your files"
check "--home: the plan, with rsync" "$R import $W/ubuntu --user alice --home > $W/home.out && grep -q 'rsync -aHAX' $W/home.out && grep -q -- '--exclude=/.cache' $W/home.out && grep -q 'Documents' $W/home.out"
if command -v rsync >/dev/null; then
  check "--home <folder>: copied, caches and what the repo makes left behind" "$R import $W/ubuntu --user alice --home $W/newhome >/dev/null 2>&1 && test -s $W/newhome/Documents/notes.txt && ! test -e $W/newhome/.cache && ! test -e $W/newhome/.bashrc && ! test -e $W/newhome/.gitconfig"
  check "Firefox's profile from its snap goes where nami's Firefox looks" "test -s $W/newhome/.mozilla/firefox/profiles.ini && ! test -e $W/newhome/snap"
else
  echo "  (no rsync here: copying not tried)"
fi

echo "== Windows"
WREPO=$W/wrepo
check "import from Windows' disk" "$R import $W/windows --out $WREPO > $W/win.out 2>&1 && test -s $WREPO/*/system.nami"
WS=$(echo $WREPO/*/system.nami)
check "its programs as Linux apps" "grep -qx '  - google-chrome' $WS && grep -qx '  - vscode' $WS && grep -qx '  - spotify' $WS && grep -qx '  - steam' $WS"
check "Rhino (no Linux version) and an unknown one: listed" "grep -q 'Windows program Rhino 8' $WS && grep -q 'Windows program Frobnicator Pro' $WS"
check "Wi-Fi: the exported copy's password, sealed; the open one; the work one listed" "grep -q 'Home & Garden:' $WS && [ \"\$($R secret show $WS password)\" = garden-pass-123 ] && grep -q 'Library' $WS && grep -q 'Wi-Fi network Corp' $WS"
check "no secret in plain text" "! grep -rq $PLAIN $WREPO $W/win.out"
check "nami check passes" "$R check $WS && $R check $WREPO/*/*/user.nami"
check "you, as an admin, on namiOS" "grep -A1 '^packages:' $WS | grep -q namios && grep -A2 ' alice:' $WS | grep -q 'admin: true'"
check "--home: a command per folder" "$R import $W/windows --home > $W/whome.out && grep -q 'Documents/ ~/Documents/' $W/whome.out && grep -q 'OneDrive/Pictures/ ~/Pictures/' $W/whome.out"
if command -v rsync >/dev/null; then
  check "--home <folder>: the folders copied" "$R import $W/windows --home $W/winhome >/dev/null 2>&1 && test -s $W/winhome/Documents/report.txt"
fi

echo
if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi
