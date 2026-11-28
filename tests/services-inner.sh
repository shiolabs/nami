#!/bin/bash
# Runs inside a container; see tests/services.sh. $1: systemd (booted with
# systemd as pid 1) or none (a plain container).
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
# What a check printed is /tmp/out afterwards, for the checks after it.
check() { if eval "$2" >/tmp/out.now 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out.now | tail -8; fi; mv -f /tmp/out.now /tmp/out; }
check_fails() { if eval "$2" >/tmp/out 2>&1; then bad "$1 (succeeded!)"; else ok "$1: $(grep -m1 error /tmp/out)"; fi; }
# busybox's wget, from the system profile: the container has no curl.
get() { /nix/var/nami/profiles/default/bin/busybox wget -q -T 3 -O - "http://127.0.0.1:$1/"; }
finish() { echo; [ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null

if [ "$1" = none ]; then
  echo "== no systemd: services wait, the rest is set up"
  printf 'packages: [hello]\nservices:\n  - web:\n      run: /bin/sleep infinity\n' > /tmp/s.yaml
  check "the rest still applies" "nami apply /tmp/s.yaml && test -x /nix/var/nami/profiles/default/bin/hello"
  check "and says the services need systemd" "grep 'services need systemd' /tmp/out && grep 'leaving out the services (web)' /tmp/out"
  mkdir -p /etc/nami
  printf 'files:\n  /etc/demo.conf: |\n    hello\n' > /etc/nami/system.nami
  check "files: still work" "nami apply && grep -x hello /etc/demo.conf"
  check "nami doctor says so" "nami doctor | grep -E 'Services · .*not supported yet'"
  finish
  exit
fi

check "systemd is pid 1" "[ \"\$(cat /proc/1/comm)\" = systemd ]"
check "nami doctor: systemd backend" "nami doctor | grep -E 'Services · systemd'"

echo "== a service of your own, and a file it serves"
mkdir -p /etc/demo /etc/nami
echo original > /etc/demo/index.html
cat > /etc/nami/system.nami <<'EOF'
packages: [busybox]
services:
  - web:
      run: busybox httpd -f -p 8080 -h /etc/demo
      env: { GREETING: hi }
      after: [network.target]
files:
  /etc/demo/index.html: |
    served by nami
  /etc/demo/settings.json:
    port: 8080
EOF
check "apply --dry-run lists the actions" "nami apply --dry-run | grep -x '  start web.service'"
check "and changes nothing" "! test -e /etc/systemd/system/web.service && grep -x original /etc/demo/index.html"
check "apply" "nami apply"
grep -E 'service|/etc/' /tmp/out | sed 's/^/       /'
check "unit installed as web.service" "grep -q 'ExecStart=/nix/store/.*busybox.*/bin/busybox httpd -f -p 8080' /etc/systemd/system/web.service"
check "enabled" "systemctl is-enabled web.service | grep -x enabled"
check "active" "systemctl is-active web.service"
check "runs as a dynamic user" "[ \"\$(ps -o user= -p \$(systemctl show -p MainPID --value web.service))\" != root ]"
check "env reaches it" "tr '\\0' '\\n' < /proc/\$(systemctl show -p MainPID --value web.service)/environ | grep -x GREETING=hi"
check "the file is there (a real file, root's)" "test -f /etc/demo/index.html && ! test -L /etc/demo/index.html && grep -x 'served by nami' /etc/demo/index.html"
check "the old one is kept" "grep -x original /etc/demo/index.html.nami-backup"
check "settings rendered as JSON" "grep '\"port\": 8080' /etc/demo/settings.json"
check "it serves the file" "sleep 1; get 8080 | grep 'served by nami'"
check "status shows it active" "nami status | grep 'web.service is running'"
check "status --json too" "nami status --json | grep -q '\"unit\":\"web.service\"' && nami status --json | grep -q '\"state\":\"active\"'"
check "status lists the files" "nami status | grep -E '[0-9]+ files? in /etc'"

echo "== change it: restarted"
pid=$(systemctl show -p MainPID --value web.service)
sed -i 's/8080 -h/8081 -h/' /etc/nami/system.nami
check "apply the change" "nami apply"
check "says it restarted" "grep 'restarted web.service' /tmp/out"
check "history diff names it" "nami diff | grep -- '~ web.service'"
check "a new process" "[ \"\$(systemctl show -p MainPID --value web.service)\" != $pid ]"
check "on the new port" "sleep 1; get 8081 | grep 'served by nami'"

echo "== undo: back"
check "undo" "nami undo"
check "old port again" "sleep 1; get 8080 | grep 'served by nami'"
check "the nami file restored too" "grep -q 'p 8080' /etc/nami/system.nami"

echo "== edited by hand, then managed on"
echo "hand edit" > /etc/demo/index.html
check "status says it changed" "nami status | grep 'index.html was changed by hand'"
sed -i 's/served by nami/served by nami, v2/' /etc/nami/system.nami
check "apply" "nami apply"
check "the hand edit was moved aside" "grep -x 'hand edit' /etc/demo/index.html.nami-backup-1"
check "and the spec's version is there" "grep -x 'served by nami, v2' /etc/demo/index.html"

echo "== its own file: restarted when it changes"
cat > /etc/nami/system.nami <<'EOF'
packages: [busybox]
services:
  - web:
      run: busybox httpd -f -p 8080 -h /etc/demo
      env: { GREETING: hi }
      after: [network.target]
      /etc/demo/settings.json:
        port: 8080
files:
  /etc/demo/index.html: |
    served by nami, v2
EOF
check "apply (the file is as it was: no restart)" "nami apply >/tmp/apply.out 2>&1 && ! grep 'restarted web.service' /tmp/apply.out"
pid=$(systemctl show -p MainPID --value web.service)
sed -i 's/port: 8080/port: 8082/' /etc/nami/system.nami
check "dry run says why" "nami apply --dry-run | grep -x '  restart web.service (/etc/demo/settings.json changed)'"
check "apply the new settings" "nami apply"
check "restarted for them" "grep 'restarted web.service' /tmp/out && [ \"\$(systemctl show -p MainPID --value web.service)\" != $pid ]"

echo "== remove it all: stopped, gone, restored"
printf 'packages: [busybox]\n' > /etc/nami/system.nami
check "apply" "nami apply"
check "says it stopped" "grep 'stopped web.service' /tmp/out"
check "inactive" "! systemctl is-active web.service"
check "unit file gone" "! test -e /etc/systemd/system/web.service"
check "not wanted any more" "! test -e /etc/systemd/system/multi-user.target.wants/web.service"
check "the original index.html is back" "grep -x original /etc/demo/index.html"
check "settings.json gone" "! test -e /etc/demo/settings.json"

echo "== undo the removal: running again"
check "undo" "nami undo"
check "active again" "systemctl is-active web.service"

echo "== a user's services (alice isn't an admin)"
useradd -m -s /bin/bash alice
loginctl enable-linger alice
for _ in $(seq 20); do [ -S /run/user/$(id -u alice)/systemd/private ] && break; sleep 0.5; done
as_alice() { su - alice -c "export XDG_RUNTIME_DIR=/run/user/\$(id -u); $1"; }
as_alice 'cat > ~/user.nami' <<'EOF'
packages: [busybox]
services:
  - syncthing
  - mine:
      run: busybox httpd -f -p 9090 -h /etc/demo
EOF
check "apply as alice" "as_alice 'nami apply'"
grep -E 'service' /tmp/out | sed 's/^/       /'
check "the package's user unit" "as_alice 'grep -q ExecStart=/nix/store/.*syncthing ~/.config/systemd/user/syncthing.service'"
check "syncthing active (systemctl --user)" "as_alice 'systemctl --user is-active syncthing.service'"
check "her own service active" "as_alice 'systemctl --user is-active mine.service'"
check "running as alice" "[ \"\$(ps -o user= -p \$(as_alice 'systemctl --user show -p MainPID --value mine.service'))\" = alice ]"
check "her status shows them" "as_alice 'nami status' | grep 'mine.service is running'"
check "not in the system's units" "! test -e /etc/systemd/system/mine.service"
as_alice "mkdir -p ~/bad ~/bad2 && printf 'services:\n  - mine:\n      run: busybox true\n      user: root\n' > ~/bad/user.nami"
check_fails "user: is for system services" "as_alice 'nami check ~/bad/user.nami && nami apply ~/bad/user.nami'"
as_alice "printf 'files:\n  /etc/evil.conf: x\n' > ~/bad2/user.nami"
check_fails "files: aren't hers to write" "as_alice 'nami apply ~/bad2/user.nami'"
check "and nothing was written" "! test -e /etc/evil.conf"
as_alice "printf 'packages: [busybox]\n' > ~/user.nami"
check "removing hers stops them" "as_alice 'nami apply' && ! as_alice 'systemctl --user is-active mine.service' && ! as_alice 'test -e ~/.config/systemd/user/mine.service'"
check "her undo starts them again" "as_alice 'nami undo' && as_alice 'systemctl --user is-active mine.service'"

echo "== Tailscale on another distro: its daemon, as nixpkgs ships it"
# (In userspace here: a container has no tun device of its own.)
cat > /etc/nami/system.nami <<'EOF2'
packages: [tailscale]
services:
  - tailscaled: { package: tailscale, env: { PORT: "41641", FLAGS: "--tun=userspace-networking" } }
EOF2
check "apply" "nami apply"
check "tailscaled active" "systemctl is-active tailscaled.service"
ts=/nix/var/nami/profiles/default/bin/tailscale
check "tailscale says it needs a login" "for i in \$(seq 30); do $ts status --json --peers=false | grep -q NeedsLogin && break; sleep 1; done; $ts status --json --peers=false | grep '\"BackendState\": \"NeedsLogin\"'"
check "its state in /var/lib/tailscale" "test -s /var/lib/tailscale/tailscaled.state"

finish
