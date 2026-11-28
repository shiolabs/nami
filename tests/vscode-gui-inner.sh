#!/bin/sh
# Runs inside an unprivileged Fedora container: install VS Code from the
# pushed cache (no building possible here), open a window, screenshot it.
set -eu
/src/install.sh /new/nami >/dev/null
. /etc/profile.d/nami.sh
nami update --from /i.xz --unsigned >/dev/null
cat > /tmp/user.nami <<EOF
caches:
  - url: file:///cache/c
    key: $(cat /cache/test.public)
packages:
  /ex/vscode:
  - xvfb-run
  - imagemagick
  - xdotool
EOF
nami apply /tmp/user.nami 2>&1 | grep -E "downloading|^\+|generation|error"
nami doctor | grep -E "Graphics|apps with windows"

mkdir -p /tmp/proj
cat > /tmp/proj/main.rs <<'EOF'
// Opened in VS Code, installed by nami from a signed cache.
// No Nix on this machine, and VS Code isn't in nixpkgs' cache (unfree).
fn main() {
    let distro = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    println!("Hej from nami on {}", distro.lines().next().unwrap_or("?"));
}
EOF
export HOME=/tmp/home
mkdir -p $HOME
xvfb-run -s "-screen 0 1280x720x24" sh -c '
  code --no-sandbox --user-data-dir /tmp/u --disable-gpu --disable-workspace-trust /tmp/proj /tmp/proj/main.rs >/tmp/code.log 2>&1 &
  sleep 25
  xdotool mousemove 1112 94 click 1   # close the welcome dialog
  sleep 3
  import -window root /out/vscode.png
'
ls -la /out/vscode.png
